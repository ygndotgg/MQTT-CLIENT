# MQTT Client From Scratch: Building a Stateful Runtime, Not Just a Codec
"An MQTT client is just packet parsing plus a socket."
That is what people usually say when they reduce MQTT to its surface area.
But honestly, that is wrong. A real MQTT client is a long-lived runtime that translates user intent into protocol packets, remembers unfinished work, survives reconnects, and, eventually, serves many logical clients over one physical connection.

NOTE: This is a technical deep-dive into `mqtt-client`, a Rust MQTT v3.1.1 client built from scratch for learning. The protocol codec in this project is derived directly from the MQTT 3.1.1 specification. The ownership model of the runtime is inspired by `rumqttc`, but the implementation here is intentionally smaller and more explicit so the architecture is easier to study.

## What This Post Covers
part i: from intent to execution - the smallest honest client architecture
part ii: protocol memory - why stateless publish is a lie
part iii: the runtime engine - parser, event loop, keepalive, reconnect
part iv: the library layer - logical clients, registry, router, fanout

## Prologue: The Stateful Client Problem
I started this project after reading the MQTT 3.1.1 specification and working through MQTT learning material. The goal was not to ship another client. The goal was to understand what an MQTT client actually is under the hood.

At first, MQTT looks small: parse packets, encode packets, write a socket loop, and move on.
That is the illusion.

The wall appears as soon as packets stop being isolated.
A publish may need a packet id. It may need an acknowledgment. It may need replay after reconnect. A subscribe cannot safely mutate local routing until `SUBACK` arrives. An unsubscribe is worse: reconnect can happen before `UNSUBACK`, so the route cannot be removed yet.

That is the story of this project.
More importantly, that is the reason this client had to be built as a runtime system rather than as a thin codec wrapper.

The repository evolved in that exact order:

- codec and strict MQTT packet support
- runtime state
- QoS1 and QoS2 tracking
- keepalive and reconnect preparation
- driver layer
- client layer
- runtime bootstrap and event-loop ownership
- transport and parser integration
- registry and targeted completion routing
- router and multi-client fanout

---
## PART I: FROM INTENT TO EXECUTION
### Chapter 1: The Smallest Useful MQTT Client
The first honest slice of the architecture is this:

```text
ClientHandle -> RuntimeTask -> Transport -> Broker
```

No reconnect.
No inflight memory.
No router.
No multi-client concerns.

Just one publish request leaving user code and becoming bytes on the wire.

#### The Problem
The naive implementation is always tempting:

```text
ClientHandle.publish(...)
  -> encode packet
  -> socket.write_all(...)
```

That fails because the API layer now owns transport. Once that happens, keepalive, reconnect, incoming packet handling, and later multi-client sharing all become harder than they need to be.

The mistake is mixing three different responsibilities into one box:

- user intent
- protocol packet construction
- transport side effects

#### The Solution: Intent First, Execution Later
The smallest correct split is:

```text
ClientHandle turns user action into Command.
RuntimeTask turns Command into Packet.
Transport turns Packet into bytes.
```

The essential types are small:

```rust
pub struct ClientHandle {
    command_tx: Sender<Command>,
    next_token_id: usize,
}

pub enum Command {
    Publish {
        token_id: usize,
        publish: Publish,
    },
}

pub struct Publish {
    pub dup: bool,
    pub qos: Qos,
    pub retain: bool,
    pub topic: String,
    pub pkid: u16,
    pub payload: Vec<u8>,
}

pub struct RuntimeTask<T: Transport> {
    command_rx: Receiver<Command>,
    transport: T,
}
```

The fields already tell the story.

`ClientHandle`
- `command_tx: Sender<Command>`
  - sends user intent to the runtime
- `next_token_id: usize`
  - creates a caller-visible request identity

`Command::Publish`
- `token_id: usize`
  - request identity
- `publish: Publish`
  - user intent in protocol-ready shape

`Publish`
- `topic: String`
- `payload: Vec<u8>`
- `qos: Qos`
- `pkid: u16` and `dup: bool`
  - mostly future-facing at this stage, but part of the final shape

`RuntimeTask`
- `command_rx: Receiver<Command>`
  - consumes intent
- `transport: T`
  - owns side effects

The first clean flow is:

```rust
// ClientHandle
let token_id = self.next_token_id;
self.next_token_id += 1;

let publish = Publish {
    dup: false,
    qos: Qos::AtMostOnce,
    retain: false,
    topic: "a/b".into(),
    pkid: 0,
    payload: b"hello".to_vec(),
};

self.command_tx.send(Command::Publish { token_id, publish })?;
```

Then the runtime takes over:

```rust
match cmd {
    Command::Publish { token_id: _, publish } => {
        let packet = Packet::Publish(publish);
        self.send_packet(packet)?;
    }
}
```

And finally:

```rust
fn send_packet(&mut self, packet: Packet) -> Result<(), RuntimeTaskError> {
    let mut bytes = Vec::new();
    encode_packet(&packet, &mut bytes)?;
    self.transport.write_all(&bytes)?;
    Ok(())
}
```

That is the first important pipeline of the whole project:

```text
intent -> command -> packet -> bytes
```

#### Step 1 Diagram

```text
+---------------------------+
| ClientHandle              |
|---------------------------|
| command_tx                |
| next_token_id             |
+---------------------------+
            |
            | Command::Publish
            v
+---------------------------+
| RuntimeTask               |
|---------------------------|
| command_rx                |
| transport                 |
+---------------------------+
            |
            | Packet::Publish -> encode
            v
+---------------------------+
| Transport                 |
+---------------------------+
            |
            | bytes
            v
         Broker
```

#### The Point
If Step 1 is correct, later features have a natural home.
If Step 1 is wrong, every later feature becomes harder.

The mental model is simple:

```text
ClientHandle owns intent.
RuntimeTask owns execution.
Transport owns bytes leaving the process.
```

### Chapter 2: The Driver Boundary
Step 1 is enough for one outgoing publish.
It is not enough for a real client.

The moment incoming packets, keepalive, reconnect, and completion routing appear, `RuntimeTask` starts knowing too much.

That leads to the next split:

```text
RuntimeTask -> RuntimeDriver -> RuntimeState
```

#### The Problem
If `RuntimeTask` handles everything directly, it becomes a giant control box:

```text
poll command
  -> inspect command
  -> mutate protocol state
  -> decide next packet
  -> maybe complete request
  -> maybe reconnect

poll incoming packet
  -> inspect packet type
  -> mutate protocol state
  -> maybe send response
  -> maybe notify caller
```

That shape fails because execution and protocol dispatch are fused together.

#### The Solution: Separate Execution, Dispatch, and Truth
The essential fields look like this:

```rust
pub struct RuntimeTask<T: Transport> {
    command_rx: Receiver<Command>,
    driver: RuntimeDriver,
    transport: T,
    parser: FrameParser,
    tick_interval: Duration,
    last_tick: Instant,
}

pub struct RuntimeDriver {
    state: RuntimeState,
    clean_session: bool,
}

pub struct RuntimeState {
    pkid_pool: PacketIdPool,
    inflight: InflightStore,
    active: bool,
    keep_alive: Duration,
    await_pingresp: bool,
    last_incoming: Instant,
    last_outgoing: Instant,
}
```

Each box now owns one kind of work.

`RuntimeTask`
- owns external inputs
- owns transport and parser
- owns time polling
- executes side effects

`RuntimeDriver`
- receives `DriverEvent`
- chooses the correct state transition
- returns `DriverAction`

`RuntimeState`
- owns packet ids
- owns inflight and pending state
- owns keepalive and reconnect truth
- decides what is legal right now

That gives the runtime a clean internal contract:

```text
RuntimeTask receives an external event
-> converts it to DriverEvent
-> RuntimeDriver chooses the transition
-> RuntimeState mutates protocol truth
-> RuntimeDriver returns DriverAction
-> RuntimeTask performs the side effect
```

In code shape, the important message types are:

```rust
pub enum DriverEvent {
    Tick(Instant),
    Command(Command),
    Incoming(Packet, Instant),
    ConnectionLost { clean_session: bool },
    ConnectionRestored(Instant),
}

pub enum DriverAction {
    Send(Packet),
    TriggerReconnect,
    CompleteFor { client_id: usize, completion: Completion },
    SubscribeAckFor { client_id: usize, filters: Vec<Filter>, completion: Completion },
    UnsubscribeAckFor { client_id: usize, filters: Vec<String>, completion: Completion },
}
```

The important idea is not the enum variants themselves.
The important idea is the boundary:

```text
Driver decides.
Task executes.
```

That prevents protocol rules from leaking into transport code.

#### Step 2 Diagram

```text
ClientHandle
    |
    | Command
    v
RuntimeTask
    |
    | DriverEvent
    v
RuntimeDriver
    |
    | state transition
    v
RuntimeState
    |
    | result
    v
RuntimeDriver
    |
    | DriverAction
    v
RuntimeTask
    |
    | encode + write
    v
Transport
    |
    v
Broker
```

#### The Point
This is the first clean shape of the runtime engine.

The mental model is:

```text
Task runs the world.
Driver chooses the transition.
State remembers the truth.
```

---
## PART II: PROTOCOL MEMORY
### Chapter 3: The Fallacy of Stateless Publish
Step 2 gave us a clean runtime boundary.
It still did not give us a real MQTT client.

The next wall appears the moment publish stops being QoS0.

For QoS1 and QoS2, a publish is not finished when bytes leave the process.
It is only finished when the broker completes the handshake.

That means a client cannot be stateless.
The protocol itself forces memory.

For QoS1, the runtime must remember:

- which packet id was used
- which logical request owns that packet id
- whether the broker has acknowledged it yet

For QoS2, it must remember even more:

- that the outgoing publish exists
- that `PUBREC` has arrived
- that `PUBREL` is now the active sender-side phase
- that terminal `PUBCOMP` is still pending

The shape of the problem is simple:

```text
PUBLISH sent
!=
operation complete
```

That is the fallacy of stateless publish.

The runtime cannot just send a packet and forget it.
It needs protocol memory.

For QoS1, the flow is:

```text
Command::Publish(qos1)
  -> allocate pkid
  -> store inflight operation
  -> send PUBLISH(pkid)
  -> wait for PUBACK(pkid)
  -> release inflight operation
  -> emit completion
```

For QoS2, the sender-side flow is:

```text
PUBLISH(pkid)
  -> PUBREC(pkid)
  -> PUBREL(pkid)
  -> PUBCOMP(pkid)
  -> complete
```

For incoming QoS2, the client also needs duplicate-suppression memory:

```text
incoming PUBLISH(pkid)
  -> first seen? deliver once and send PUBREC
  -> duplicate? send PUBREC again, do not redeliver
```

That is why `RuntimeState` owns packet ids, inflight state, pending replay, and phase markers.

The important lesson is:

```text
QoS is not just a packet format difference.
QoS is a memory requirement.
```

### Chapter 4: The Inflight Store
The first true memory object in this project is not the client handle.
It is `OutgoingOp`.

That is the record the runtime stores when an operation is accepted but not yet complete.

The essential types are:

```rust
pub struct OutgoingOp {
    pub token_id: usize,
    pub command: Command,
    pub packet: Packet,
    pub client_id: usize,
}

pub struct InflightStore {
    pub outgoing: Vec<Option<OutgoingOp>>,
    pub inflight: u16,
    pub pending: Vec<OutgoingOp>,
    pub collision: Option<(u16, OutgoingOp)>,
    pub outgoing_rel: Vec<bool>,
    pub incoming_pub: Vec<bool>,
}
```

The roles are precise.

`OutgoingOp`
- `token_id`
  - which logical request this will eventually complete
- `client_id`
  - which logical client owns it
- `command`
  - original intent, needed for replay and ack interpretation
- `packet`
  - last built wire packet

This one struct is why later features work:

- targeted completion routing
- reconnect replay
- subscribe ownership
- unsubscribe ownership

`InflightStore`
- `outgoing[pkid]`
  - active QoS1/QoS2 operations waiting for ack
- `pending`
  - accepted work that cannot be sent yet or must be replayed later
- `collision`
  - one deferred op blocked by an occupied packet-id slot
- `outgoing_rel`
  - sender-side QoS2 `PUBREC -> PUBREL` phase marker
- `incoming_pub`
  - receiver-side QoS2 duplicate-suppression marker

The existence of `pending` is just as important as `outgoing`.

`outgoing` means:

```text
sent and waiting
```

`pending` means:

```text
accepted by runtime, but not yet safely completed on wire
```

That distinction is what makes reconnect replay possible.

The packet-id allocator exists because tracked MQTT operations cannot use packet id `0`.
So the runtime needs a place that can hand out non-zero candidates before storing a tracked operation.

The QoS1 path in distilled form looks like this:

```text
on_command_publish(qos1)
  -> allocate pkid
  -> build OutgoingOp
  -> insert into outgoing[pkid]
  -> send PUBLISH(pkid)

on_puback_checked(pkid)
  -> release outgoing[pkid]
  -> recover token_id and client_id from OutgoingOp
  -> emit completion
```

That is the key memory trick:

```text
ack routing works because ownership was stored when the command was first accepted
```

The reconnect path uses the same stored object:

```text
disconnect
  -> move active ops into pending

restore
  -> rebuild packets from pending OutgoingOp entries
  -> resend them
```

For QoS1 and QoS2 publish replay, `dup = true` must be visible on the wire.

This is the first point where the project stops behaving like "send a packet" code and starts behaving like a long-lived system.

#### Part II Diagram

```text
Command::Publish
        |
        v
   RuntimeState
      /    \
     v      v
PacketIdPool  OutgoingOp
              - token_id
              - client_id
              - command
              - packet
                   |
                   v
             InflightStore
     outgoing / pending / collision
                   |
                   v
      PUBACK / PUBREC / PUBCOMP
                   |
                   v
              RuntimeState
                   |
                   v
               Completion
```

#### The Point
The most important sentence in Part II is this:

```text
QoS is protocol memory.
```

If the client cannot remember unfinished operations, it cannot implement QoS1, QoS2, or reconnect replay correctly.

---
## PART III: THE RUNTIME ENGINE
### Chapter 5: The Illusion of the Socket Loop
By this point the client already has:

- intent boundaries
- driver/state separation
- protocol memory

Now the next illusion appears:

```text
just read from the socket and decode packets
```

That sounds reasonable until you remember what TCP actually gives you:

- half a packet
- exactly one packet
- multiple packets in one read

Transport is a byte stream, not a packet stream.

That is why one runtime owner must own stream progress.

The essential fields in `RuntimeTask` are:

```rust
pub struct RuntimeTask<T: Transport> {
    command_rx: Receiver<Command>,
    driver: RuntimeDriver,
    transport: T,
    parser: FrameParser,
    read_buf: [u8; 4096],
    tick_interval: Duration,
    last_tick: Instant,
}
```

The important ones for this chapter are:

- `transport: T`
  - runtime owns real I/O
- `parser: FrameParser`
  - runtime owns fragmented stream assembly
- `read_buf: [u8; 4096]`
  - runtime owns temporary read storage

The key method is conceptually:

```rust
fn read_one_packet(&mut self) -> Result<Option<Packet>, RuntimeTaskError> {
    let n = self.transport.read(&mut self.read_buf)?;
    if n == 0 {
        return Ok(None);
    }

    self.parser.push(&self.read_buf[..n]);
    self.parser.next_packet()
}
```

That one method captures the real boundary:

```text
bytes -> frame parser -> Packet
```

And that is why the socket loop is an illusion.
You are not looping over packets.
You are managing stream assembly until a packet becomes available.

The runtime must own that work because it is the same layer that also owns:

- command polling
- keepalive ticks
- reconnect
- outgoing packet writes

If parser progress lives somewhere else, the execution model fragments.

#### Step 3 Diagram

```text
Broker
  |
  | bytes
  v
Transport
  |
  | read chunk
  v
read_buf
  |
  v
FrameParser
  |
  | full frame
  v
Packet
  |
  v
RuntimeTask
  |
  v
RuntimeDriver
  |
  v
RuntimeState
```

#### The Point
The runtime does not read packets.
It reads bytes until a packet can exist.

That distinction is what gives `RuntimeTask` a real reason to own transport and parser together.

### Chapter 6: The Event Loop
Once the runtime owns:

- commands
- packet decoding
- protocol state
- outgoing writes

it stops being a helper and becomes an event loop.

The loop in the current code is simple:

```text
loop {
  poll_command()
  poll_incoming()
  poll_tick()
}
```

That simplicity is useful.
It makes the ownership obvious.

The runtime has three upstreams:

```text
1. command channel
2. incoming transport bytes
3. time
```

All three converge into the same owner:

```text
RuntimeTask
```

The three pollers have distinct jobs.

`poll_command()`
- consumes `Command`
- converts it into `DriverEvent::Command`
- asks the driver what should happen next

`poll_incoming()`
- turns bytes into `Packet`
- converts it into `DriverEvent::Incoming`
- asks the driver, or the special publish path, what should happen next

`poll_tick()`
- checks elapsed time
- emits `DriverEvent::Tick(now)`
- lets state decide whether to do nothing, send `PINGREQ`, or reconnect

The keepalive path is the first place where time becomes protocol logic:

```text
idle too long
  -> send PINGREQ

waiting too long for PINGRESP
  -> trigger reconnect
```

The reconnect path is where all previous layers come together:

```text
tick timeout or transport failure
  -> DriverAction::TriggerReconnect
  -> RuntimeTask broadcasts Disconnected
  -> RuntimeTask emits ConnectionRestored into driver
  -> RuntimeState replays pending work
  -> RuntimeTask sends replay packets
  -> RuntimeTask broadcasts Reconnected
```

The replay matters because protocol memory is now part of execution.

If pending work exists, reconnect is not "reconnect and continue."
It is:

```text
reconnect
-> restore protocol truth
-> resend unfinished operations
```

For QoS1 and QoS2 publish replay, the runtime marks the publish with `dup = true`.

This is the first point where the whole architecture clicks into one sentence:

```text
Task owns progress.
Driver owns decisions.
State owns memory.
```

#### Part III Diagram

```text
Command channel ----\
                     \
Transport bytes ------> RuntimeTask ----> RuntimeDriver ----> RuntimeState
                     /          ^                 |
Time / Tick --------/           |                 |
                                |                 |
                                +---- DriverAction+
                                          |
                                          v
                                   RuntimeTask side effects
                                   - Send Packet -> Transport
                                   - Emit RuntimeEvent
                                   - Reconnect
```

#### The Point
The event loop is not a convenience wrapper around the client.
It is the client.

Everything else in the architecture exists to keep that one owner coherent.

---
## PART IV: THE LIBRARY ARCHITECTURE
### Chapter 7: The Dimension of Logical Clients
Up to this point, the architecture describes one runtime and one caller.
That is enough to explain protocol correctness.
It is not enough to explain a client library.

A library has a second job:

```text
expose one connection to many logical users without losing ownership clarity
```

That is where the design becomes multi-client.

The key split is:

```text
many ClientHandle
        |
        v
   one RuntimeHost
        |
        v
   one RuntimeTask
```

The important type is small:

```rust
pub struct RuntimeHost {
    command_tx: Sender<Command>,
    registry: Arc<Mutex<ClientRegistry>>,
}
```

`RuntimeHost` exists for one reason:

- runtime starts once
- logical clients can be created many times later

That leads to the key method:

```rust
pub fn new_client(&self) -> ClientHandle {
    let (event_tx, event_rx) = channel();
    let client_id = self.registry.lock().unwrap().register(event_tx);
    ClientHandle::new(client_id, self.command_tx.clone(), event_rx)
}
```

This is the library-side ownership model in one place.

Each logical client gets:

- shared `command_tx`
- private `event_rx`
- unique `client_id`

That means:

```text
commands are shared
events are isolated
```

This is the right design because the physical connection is shared, but the caller-facing event stream is not.

This part of the design was strongly influenced by `rumqttc`.
Not by copying its code directly, but by following the same core ownership question:

```text
how do many logical clients share one connection without breaking protocol ownership?
```

#### Part IV-A Diagram

```text
                 +-------------------+
                 | RuntimeHost       |
                 +-------------------+
                  /        |        \
                 v         v         v
        +-------------+ +-------------+ +-------------+
        | Client A    | | Client B    | | Client C    |
        | event_rx A  | | event_rx B  | | event_rx C  |
        +-------------+ +-------------+ +-------------+
               \             |             /
                \            |            /
                 \           |           /
                  +---------------------+
                  | shared command_tx   |
                  v
            +-------------------+
            | RuntimeTask       |
            +-------------------+

RuntimeTask sends events back through each client's private event_rx.
```

#### The Point
Multi-client architecture is not multiple runtimes.
It is multiple logical callers over one runtime owner.

### Chapter 8: The Router Vault
Once multiple logical clients exist, the runtime must answer two different routing questions:

```text
1. Which client owns this completion?
2. Which clients should receive this incoming publish?
```

Those are solved by two different structures.

`ClientRegistry`

```rust
pub struct ClientRegistry {
    next_id: usize,
    event_txs: HashMap<usize, Sender<RuntimeEvent>>,
}
```

It answers:

```text
client_id -> where do I send the event?
```

`Router`

```rust
pub struct SubscriptionEntry {
    pub filter: String,
    pub clients: Vec<usize>,
}

pub struct Router {
    entries: Vec<SubscriptionEntry>,
}
```

It answers:

```text
topic filter -> which logical clients should receive this publish?
```

That separation is the whole trick.

`ClientRegistry` is mailbox routing.
`Router` is publish fanout routing.

They are related, but they are not the same.

The second important rule is when the router is allowed to change.

This client follows an intentionally conservative policy:

```text
add route on SUBACK
remove route on UNSUBACK
```

Not on `SUBSCRIBE` send.
Not on `UNSUBSCRIBE` send.
Not on reconnect alone.

That policy matters because local routing should follow broker-confirmed truth, not optimistic intent.

So the subscribe path is:

```text
Command::Subscribe
  -> send SUBSCRIBE
  -> wait for SUBACK
  -> router.add_subscription(...)
```

And unsubscribe is:

```text
Command::Unsubscribe
  -> send UNSUBSCRIBE
  -> wait for UNSUBACK
  -> router.remove_subscription(...)
```

This is especially important during reconnect replay.
If unsubscribe is replayed after reconnect, the route must stay alive until `UNSUBACK` actually arrives.

This is also one of the clearest places where the project aligns with `rumqttc`.
The code is different, but the ownership idea is the same:

```text
protocol truth and library routing truth must stay separate
```

#### Part IV-B Diagram

```text
SUBACK / UNSUBACK
        |
        v
   RuntimeTask
    /       \
   v         v
Router    ClientRegistry
filter ->  client_id ->
client_ids mailbox
   |           |
   v           v
Incoming     RuntimeEvent
Publish      delivery
fanout
```

#### The Point
The client library works because it keeps these two truths separate:

```text
ClientRegistry decides where an event is delivered.
Router decides who should receive an incoming publish.
```

### Chapter Final: The Code Architecture
At the end of the journey, the project is best described like this:

```text
ClientHandle expresses intent.
RuntimeHost creates logical clients.
RuntimeTask owns live execution.
RuntimeDriver dispatches events.
RuntimeState owns protocol truth.
InflightStore remembers unfinished work.
Router owns incoming publish audience.
ClientRegistry owns event destinations.
```

That is the full shape of the system.

In one diagram:

```text
ClientHandle(s)
      |
      v
RuntimeHost
      |
      v
RuntimeTask
  |    |    |    \
  |    |    |     \
  v    v    v      v
Driver Transport  ClientRegistry  Router
  |    + FrameParser
  v
RuntimeState
  |
  v
InflightStore + PacketIdPool

Transport + FrameParser <-> Broker
```

The architectural lesson of the whole project is simple:

```text
MQTT is not difficult because packets are hard.
MQTT is difficult because ownership over time is hard.
```

That is why the codebase had to evolve in the order it did:

- codec
- protocol state
- runtime owner
- event loop
- reconnect replay
- library routing
- multi-client fanout

---
## Epilogue: The Rust Advantage
Rust did not automatically make this architecture good.
But it forced every ownership boundary to become explicit.

That is exactly why this project is useful as a systems exercise.

If I had to summarize the project in one line, it would be this:

```text
the MQTT 3.1.1 spec gave the codec its rules, and rumqttc gave the runtime a practical ownership shape to learn from
```

The Code:
https://github.com/ygndotgg/MQTT-CLIENT

MQTT Client From Scratch: Building a Stateful Runtime, Not Just a Codec | my thoughts
