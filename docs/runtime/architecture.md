# Runtime Architecture

This document explains the current runtime architecture in this repository, what is already implemented, and how the full multi-client runtime behaves end to end.

It focuses on ownership boundaries and on the exact flow of commands, acks, publishes, reconnect, and replay.

## 1) The architectural shift

Early in the project, the main problem was:

```text
bytes <-> packet structs
```

Now the repository solves a larger problem:

```text
logical client intent <-> runtime state machine <-> transport <-> broker
```

That is the shift from codec work to client-runtime work.

- codec solves wire format
- runtime solves behavior over time
- router solves library-side fanout
- client API solves ownership and isolation between logical clients

## 2) Current system shape

Today, the system looks like this:

```text
ClientHandle (many)
   |
   | shared Command channel
   v
RuntimeHost
   |
   | starts one RuntimeTask thread
   v
RuntimeTask
   |
   +-- owns Transport
   +-- owns FrameParser
   +-- owns Router
   +-- owns ClientRegistry
   +-- owns RuntimeDriver
   |
   v
RuntimeDriver
   |
   v
RuntimeState
   |
   v
InflightStore / PacketIdPool
```

And below that:

```text
Packet <-> codec <-> bytes
```

So the repository is no longer only a parser/encoder. It is a single-connection, multi-logical-client MQTT runtime.

## 3) Ownership boundaries

The most important architectural rule is that each layer owns a distinct kind of truth.

## 3.1 `ClientHandle` owns user intent, not protocol state

`src/client.rs`

Each logical client handle owns:

- `client_id`
- shared `command_tx`
- unique `event_rx`
- local token counter

It is intentionally thin.

It does:
- build commands
- attach `client_id`
- attach `token_id`
- receive per-client runtime events

It does not:
- own transport
- own parser
- own inflight tracking
- mutate MQTT protocol state
- decide publish fanout

Think of it as:

```text
I am one caller attached to the shared runtime.
I can express intent and receive my own events.
```

## 3.2 `RuntimeHost` owns runtime bootstrap and logical-client creation

`src/runtime/host.rs`

`RuntimeHost` starts the one runtime task and creates more logical clients later.

It owns:

- shared command sender
- shared `ClientRegistry`

Its job is:

1. create the shared runtime once
2. create per-client event mailboxes on demand
3. return `ClientHandle`s bound to the same runtime

This means:

```text
many ClientHandle
        |
        v
   one RuntimeTask
```

That is the multi-client architecture.

## 3.3 `RuntimeTask` owns real execution and library-side routing

`src/runtime/task.rs`

`RuntimeTask` is the single mutable owner of live runtime execution.

It owns:

- command polling
- incoming transport polling
- tick polling
- parser
- router
- client registry access
- reconnect orchestration
- packet send path

This is the layer that turns abstract driver actions into actual side effects.

Examples:

- `Send(Packet)` -> encode and write bytes
- `CompleteFor { client_id, ... }` -> route completion to one client mailbox
- `SubscribeAckFor { ... }` -> mutate router, then emit completion
- `TriggerReconnect` -> broadcast `Disconnected`, replay pending work, broadcast `Reconnected`

Think of it as:

```text
I am the live engine for one connection and many logical clients.
```

## 3.4 `RuntimeDriver` owns event dispatch, not transport or fanout

`src/runtime/driver.rs`

`RuntimeDriver` receives `DriverEvent` and decides which `RuntimeState` method should run.

It owns:

- coordination
- event-to-state dispatch
- conversion from state results into `DriverAction`

It does not own:

- socket reads/writes
- parser buffering
- router mutation
- client mailbox routing

Think of it as:

```text
Given this external event, which state transition should happen next?
```

## 3.5 `RuntimeState` owns MQTT protocol truth

`src/runtime/state.rs`

`RuntimeState` is the durable protocol brain.

It owns:

- packet-id allocation
- inflight bookkeeping
- pending replay queue
- QoS1 completion logic
- outgoing QoS2 state
- incoming QoS2 duplicate suppression
- keepalive timers
- reconnect cleanup and replay preparation

It does not own:

- client event channels
- topic-based publish fanout
- transport writes

Think of it as:

```text
What is legal right now, and what protocol state change follows from this event?
```

## 3.6 `Router` owns library-side publish fanout, not MQTT session state

`src/runtime/router.rs`

`Router` stores:

- subscription filter entries
- logical clients attached to each filter

It answers one question:

```text
For this incoming publish topic, which logical clients should receive it?
```

It is not protocol truth.

MQTT itself only requires that the broker and client exchange packets correctly.
The router exists because this library exposes one connection to many logical client handles.

## 3.7 `ClientRegistry` owns logical client mailboxes

`src/runtime/task.rs`

`ClientRegistry` stores:

```text
client_id -> Sender<RuntimeEvent>
```

It lets the runtime say:

- send to exactly one logical client
- broadcast to all logical clients

That is how targeted completion routing and connection-wide reconnect notifications coexist.

## 4) Why this architecture exists

MQTT is not just a packet parsing problem.

The difficult part is:

- mapping app requests to wire packets
- remembering which request is waiting for which ack
- reconnecting without losing ownership of outstanding work
- suppressing duplicate QoS2 delivery
- fanning incoming publishes out to the correct logical clients
- delaying router mutation until the broker actually acknowledges subscribe/unsubscribe

Without runtime architecture, you only have:

```text
packet in -> packet out
```

With runtime architecture, you have:

```text
logical client issues subscribe
-> runtime sends SUBSCRIBE
-> broker SUBACK arrives
-> route is installed
-> incoming publish fans out only to matching clients
-> unsubscribe is sent
-> reconnect happens before UNSUBACK
-> unsubscribe is replayed
-> route remains until broker UNSUBACK arrives
-> route is removed
```

That memory across time is what makes this a real client library rather than a codec.

## 5) What is already implemented

## 5.1 Codec and parser

Implemented:

- packet encode/decode
- fixed header handling
- frame parsing
- fragmented stream support via `FrameParser`

## 5.2 Delivery state

Implemented:

- packet-id allocation
- inflight store
- pending replay queue
- QoS1 sender tracking
- QoS2 sender tracking
- QoS2 receiver duplicate suppression

## 5.3 Liveness and reconnect core

Implemented:

- keepalive state
- `PINGREQ` / `PINGRESP` handling
- disconnect cleanup
- replay preparation on restore
- replay of publish, subscribe, and unsubscribe operations

Current limitation:
- reconnect restores protocol state and replay path, but does not yet recreate a real external socket connection object

## 5.4 Multi-client library layer

Implemented:

- `RuntimeHost`
- many `ClientHandle`s over one runtime
- `ClientRegistry`
- targeted completion routing
- `Router`
- subscription-based incoming publish fanout
- route installation on `SUBACK`
- route removal on `UNSUBACK`
- disconnect/reconnect broadcast notifications

## 5.5 Integration coverage

Integration tests now cover:

- subscribe ownership and route install
- unsubscribe ownership and route removal
- targeted publish completion routing
- publish fanout by topic filter
- reconnect broadcast to all logical clients
- replayed publish completion after reconnect
- replayed unsubscribe with delayed route removal

## 6) End-to-end flow model

The current runtime has three input sources:

```text
1. command channel
2. transport byte stream
3. periodic tick
```

Each loop iteration does:

```text
poll_command()
poll_incoming()
poll_tick()
```

Each source becomes `DriverEvent`, which becomes `DriverAction`, which the task executes.

That is the core runtime pattern.

## 7) Detailed end-to-end flows

## 7.1 Client creation flow

```text
RuntimeHost::new()
  -> create shared command channel
  -> create shared ClientRegistry
  -> start one RuntimeTask thread

RuntimeHost::new_client()
  -> create event channel
  -> register sender in ClientRegistry
  -> allocate client_id
  -> return ClientHandle { client_id, shared command_tx, event_rx }
```

Meaning:

- commands flow into one runtime
- events flow back out through per-client mailboxes

## 7.2 Outgoing subscribe flow

```text
ClientHandle::subscribe(filter)
  -> Command::Subscribe { client_id, token_id, subscribe }
  -> RuntimeTask.poll_command()
  -> RuntimeDriver::handle_event(Command(...))
  -> RuntimeState::on_command_subscribe(...)
  -> OutgoingOp { client_id, token_id, command, packet }
  -> DriverAction::Send(Packet::Subscribe)
  -> RuntimeTask::send_packet(...)
```

At this point, the router is still unchanged.

That is intentional.

The library does not install routes optimistically on command send.
It waits for broker acknowledgment.

## 7.3 `SUBACK` flow and route installation

```text
broker SUBACK(pkid)
  -> RuntimeTask.poll_incoming()
  -> RuntimeDriver::handle_event(Incoming(SubAck, now))
  -> RuntimeState::on_suback_checked(pkid)
  -> SubscribeAckResult { client_id, filters, completion }
  -> DriverAction::SubscribeAckFor { client_id, filters, completion }
  -> RuntimeTask.handle_actions(...)
  -> router.add_subscription(client_id, filter)
  -> send RuntimeEvent::Completion(Completion::SubAck { ... }) to that client only
```

Meaning:

- protocol state confirms the subscribe really completed
- runtime task installs router entries only after that point
- completion remains targeted to the originating client

## 7.4 Incoming publish fanout flow

```text
broker PUBLISH(topic=t)
  -> RuntimeTask.read_one_packet()
  -> RuntimeTask.handle_incoming_publish(...)
  -> QoS handling happens first
  -> Router::matching_clients(t)
  -> for each matching client_id:
       ClientRegistry::send_to(client_id, RuntimeEvent::IncomingPublish(...))
```

This is the current publish-delivery policy:

- accept at protocol layer first
- then fan out at library layer

QoS-specific behavior:

- QoS0:
  - deliver immediately to matching clients
- QoS1:
  - deliver immediately to matching clients
  - send `PUBACK`
- QoS2:
  - use state duplicate suppression first
  - only first-seen publish is fanned out
  - duplicates are acknowledged with `PUBREC` but not delivered again

## 7.5 Outgoing publish completion flow

```text
client A publishes QoS1
  -> RuntimeState stores OutgoingOp { client_id: A, token_id, ... }
  -> broker PUBACK(pkid)
  -> RuntimeState::on_puback_checked(pkid)
  -> AckResult { client_id: A, completion, next_packet }
  -> DriverAction::CompleteFor { client_id: A, completion }
  -> RuntimeTask::send_completion_to(A, completion)
  -> only client A receives RuntimeEvent::Completion(...)
```

This is the key multi-client completion rule:

- `Completion` itself stays protocol-facing
- ownership comes from `OutgoingOp.client_id`
- routing happens in `RuntimeTask`, not in the client handle

## 7.6 Outgoing unsubscribe flow

```text
ClientHandle::unsubscribe(filter)
  -> Command::Unsubscribe { client_id, token_id, unsubscribe }
  -> RuntimeState::on_command_unsubscribe(...)
  -> OutgoingOp { client_id, token_id, command, packet }
  -> DriverAction::Send(Packet::UnSubscribe)
  -> RuntimeTask::send_packet(...)
```

Again, the router is still unchanged here.

Removing the route on send would be wrong because the broker has not acknowledged the unsubscribe yet.

## 7.7 `UNSUBACK` flow and route removal

```text
broker UNSUBACK(pkid)
  -> RuntimeTask.poll_incoming()
  -> RuntimeDriver::handle_event(Incoming(UnsubAck, now))
  -> RuntimeState::on_unsuback_checked(pkid)
  -> UnsubscribeAckResult { client_id, filters, completion }
  -> DriverAction::UnsubscribeAckFor { client_id, filters, completion }
  -> RuntimeTask.handle_actions(...)
  -> router.remove_subscription(client_id, filter)
  -> send RuntimeEvent::Completion(Completion::UnsubAck { ... }) to that client only
```

Meaning:

- route removal is ack-driven
- the router changes only after the broker confirms the unsubscribe

This is the correct library policy.

## 7.8 Tick and reconnect trigger flow

```text
tick fires
  -> RuntimeTask.poll_tick()
  -> RuntimeDriver::handle_event(Tick(now))
  -> RuntimeState::on_tick(now)
```

Possible outcomes:

- no action
- `Send(PingReq)`
- `TriggerReconnect`

If reconnect is triggered:

```text
DriverAction::TriggerReconnect
  -> RuntimeTask broadcasts Disconnected to all logical clients
  -> RuntimeTask::reconnect()
  -> RuntimeDriver::handle_event(ConnectionRestored(now))
  -> RuntimeState::on_connection_restored()
  -> replay packets are returned
  -> RuntimeTask sends replay packets
  -> RuntimeTask broadcasts Reconnected to all logical clients
```

Why disconnect/reconnect are broadcast:
- the transport connection is shared by all logical clients
- connection lifecycle is not owned by one client

## 7.9 Replayed publish flow after reconnect

If a QoS1 or QoS2 publish was outstanding at disconnect time:

```text
disconnect happens
  -> RuntimeState::on_connection_lost(...)
  -> inflight op moves to pending

restore happens
  -> RuntimeState::resend_pending_on_reconnect()
  -> publish is rebuilt
  -> QoS1/QoS2 publish is marked dup = true
  -> packet is sent again
```

Then, when broker acknowledges it:

```text
PUBACK / PUBCOMP arrives
  -> ownership still resolves through the original client_id stored in OutgoingOp
  -> completion is routed only to that owning client
```

So reconnect replay does not break targeted completion routing.

## 7.10 Replayed unsubscribe flow after reconnect

This is the subtle multi-client case.

If unsubscribe is sent but not yet acknowledged when disconnect happens:

```text
client A sends UNSUBSCRIBE(sensor/+)
  -> OutgoingOp stored with client_id A
  -> route is still present in Router

disconnect happens before UNSUBACK
  -> RuntimeState::on_connection_lost(...)
  -> unsubscribe op moves to pending
  -> task broadcasts Disconnected

restore happens
  -> RuntimeState::resend_pending_on_reconnect()
  -> unsubscribe packet is replayed
  -> task broadcasts Reconnected
```

At this point:

- route is still present
- client A may still receive matching publishes
- this is correct because the unsubscribe is not yet acknowledged

Only when broker later sends `UNSUBACK`:

```text
UNSUBACK arrives
  -> state resolves owning client and filters
  -> task removes router entry for client A
  -> task sends Completion::UnsubAck only to client A
```

After that point:

- client A stops receiving the filter
- other clients that are still subscribed continue receiving it

This is the exact behavior exercised by the reconnect-focused integration test.

## 8) Router mutation policy

The current router policy is intentionally conservative.

Rules:

- add routes on `SUBACK`
- remove routes on `UNSUBACK`
- do not add routes on `SUBSCRIBE` send alone
- do not remove routes on `UNSUBSCRIBE` send alone
- do not mutate routes on disconnect alone
- do not mutate routes on replay alone

Why this policy is correct:

- it prevents optimistic route installation
- it prevents early route removal
- it keeps router state aligned with broker-confirmed subscription state
- it continues to behave correctly when reconnect interrupts subscribe or unsubscribe mid-flight

## 9) What remains incomplete

The runtime is functionally coherent, but one major area is still simplified.

Still simplified:

- reconnect does not yet recreate a real external socket/transport object
- the runtime loop is polling-based rather than async `select!`
- transport replacement/backoff is not yet fully modeled

Already correct despite that limitation:

- ownership boundaries
- targeted completion routing
- publish fanout by topic filter
- ack-driven route mutation
- reconnect replay semantics for publish, subscribe, and unsubscribe

## 10) Relationship to `rumqttc`

This repository is moving toward the same ownership model as `rumqttc`, but in a smaller and easier-to-reason-about form.

Conceptual mapping:

```text
This repo                 rumqttc
---------                 -------
ClientHandle        ~=    Client
RuntimeHost         ~=    client/bootstrap side
RuntimeTask         ~=    Eventloop
RuntimeState        ~=    MqttState
transport/parser    ~=    Connection
Router/registry     ~=    library-side multiplexing and routing
```

The important architectural match is:

- API layer is thin
- one runtime owner holds protocol state
- transport is hidden behind runtime
- reconnect is centralized
- logical clients are a library concern layered above the one connection

## 11) Mental model to keep

If you need one sentence that describes the current system, use this:

```text
State decides protocol transitions.
Driver routes events into state.
Task owns transport, routing, and replay side effects.
Host creates logical clients.
Client only submits intent and receives its own events.
```
