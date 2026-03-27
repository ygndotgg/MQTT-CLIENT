# Runtime Event Loop

This document explains how the current runtime loop works, which layers own which responsibilities, and how commands, network traffic, timer ticks, and reconnects move through the system.

## 1) The runtime as a pipeline

At the highest level, the runtime is a pipeline with four upstreams:

```text
client commands
broker/network bytes
timer ticks
reconnect lifecycle
```

All of them converge into the same runtime owner:

```text
ClientHandle / Transport / Time
            |
            v
        RuntimeTask
            |
            v
       RuntimeDriver
            |
            v
        RuntimeState
            |
            v
   side effects + RuntimeEvent
```

The important split is:

- `RuntimeTask` owns the loop and side effects
- `RuntimeDriver` routes events
- `RuntimeState` decides protocol legality and state transitions

## 2) Ownership model

## 2.1 ClientHandle

`src/client.rs`

`ClientHandle` is intentionally thin.

It owns:

- `Sender<Command>`
- `Receiver<RuntimeEvent>`

It does not own:

- transport
- parser
- inflight state
- keepalive
- reconnect logic

So the client layer only says:

```text
send this command
wait for this runtime event
```

## 2.2 RuntimeTask

`src/runtime/task.rs`

`RuntimeTask` is the single mutable owner of runtime machinery.

It currently owns:

- `command_rx`
- `event_tx`
- `driver`
- `transport`
- `FrameParser`
- tick interval and tick bookkeeping

This is the first place where the architecture becomes real:

```text
ClientHandle -> channel -> RuntimeTask -> driver/state -> transport/events
```

## 2.3 RuntimeDriver

`src/runtime/driver.rs`

`RuntimeDriver` accepts `DriverEvent` and returns `DriverAction`.

It knows:

- which runtime-state method matches a given event
- when to emit follow-up packets
- when a completion should be surfaced
- when reconnect should start

It does not know:

- how bytes are read from the socket
- how packets are encoded
- how runtime events are sent to the client

## 2.4 RuntimeState

`src/runtime/state.rs`

`RuntimeState` is the protocol truth.

It holds:

- packet-id allocation
- inflight/pending memory
- QoS1/QoS2 tracking
- keepalive timers
- reconnect cleanup state

This is where the MQTT rules live.

## 3) The four upstreams

## 3.1 Command upstream

Path:

```text
ClientHandle.publish(...)
  -> Command::Publish
  -> command channel
  -> RuntimeTask::poll_command()
  -> RuntimeDriver::handle_event(DriverEvent::Command)
  -> RuntimeState::on_command_publish(...)
  -> DriverAction::Send(Packet::Publish)
  -> RuntimeTask::send_packet(...)
  -> transport.write_all(bytes)
```

What happens under the hood:

1. user calls `ClientHandle.publish(...)`
2. a `Command::Publish` enters the command channel
3. `RuntimeTask` polls the channel without blocking the whole loop
4. driver forwards the command to `RuntimeState`
5. state allocates `pkid` when required, tracks inflight work, and returns a wire packet
6. task encodes the `Packet` and writes bytes to transport

This is the outgoing request path.

## 3.2 Network upstream

Path:

```text
transport.read(bytes)
  -> RuntimeTask::read_one_packet()
  -> FrameParser buffers chunks
  -> FrameParser finds one full MQTT frame
  -> decode_packet(frame)
  -> Packet
  -> RuntimeTask::poll_incoming()
  -> RuntimeDriver::handle_event(DriverEvent::Incoming)
  -> RuntimeState transition
  -> DriverAction / RuntimeEvent
```

This path matters because the transport is a byte stream, not a packet stream.

That means:

- one read can contain half a packet
- one read can contain exactly one packet
- one read can contain multiple packets

So the parser boundary is:

```text
stream bytes -> full MQTT frame -> Packet
```

Only after a full frame exists should `decode_packet(...)` run.

## 3.3 Tick upstream

Path:

```text
RuntimeTask::poll_tick()
  -> Instant::now()
  -> DriverEvent::Tick(now)
  -> RuntimeState::on_tick(now)
  -> one of:
       no action
       Send(PingReq)
       TriggerReconnect
```

Why this is split this way:

- task owns the time source
- driver converts time into a driver event
- state decides keepalive semantics

`tick_interval` is not the MQTT keepalive itself.  
It is only how often the runtime checks whether keepalive work is needed.

## 3.4 Reconnect upstream

Path:

```text
I/O failure or ping timeout
  -> DriverAction::TriggerReconnect
  -> RuntimeTask emits RuntimeEvent::Disconnected
  -> RuntimeTask::reconnect()
  -> DriverEvent::ConnectionRestored(now)
  -> RuntimeState::on_connection_restored()
  -> pending packets replayed
  -> RuntimeTask emits RuntimeEvent::Reconnected
```

In the current implementation, reconnect is still a hook.

What is already correct:

- reconnect remains outside `RuntimeState`
- state handles cleanup and replay preparation
- task handles reconnect orchestration and event emission

What is still simplified:

- no real transport teardown/recreation yet
- no retry/backoff policy yet

## 4) RuntimeTask internals

`src/runtime/task.rs`

The current task loop is:

```text
loop {
  poll_command()
  poll_incoming()
  poll_tick()
}
```

This is a simple polling loop, but the structure is correct.

## 4.1 poll_command

Purpose:

- check whether a client command is waiting
- do not block the network/timer sides

That is why it uses `try_recv()` instead of blocking `recv()`.

If it blocked, the runtime would stop processing:

- incoming broker packets
- keepalive checks
- reconnect work

## 4.2 read_one_packet

Purpose:

- read bytes from transport
- push them into the frame parser
- decode one packet only when a full frame exists

Conceptually:

```text
chunk #1 -> partial -> None
chunk #2 -> still partial -> None
chunk #3 -> full frame -> Some(Packet)
```

This is how the runtime survives stream fragmentation.

## 4.3 poll_incoming

Purpose:

- fetch one decoded packet from `read_one_packet()`
- route it either through:
  - the special incoming publish path, or
  - the normal driver path

Incoming `Publish` is special because it is both:

- a protocol packet that may need acking
- an application payload that may need `RuntimeEvent::IncomingPublish`

That is why `handle_incoming_publish(...)` exists.

## 4.4 send_packet

Purpose:

- convert `Packet` into bytes
- write those bytes to the transport
- update outgoing-activity time

Conceptually:

```text
Packet -> encode_packet(...) -> bytes -> transport.write_all(...)
```

This is the mirror image of `read_one_packet()`.

## 5) Incoming publish behavior

Incoming publish handling is where protocol flow and app delivery meet.

## 5.1 QoS0

```text
incoming PUBLISH(qos0)
  -> emit RuntimeEvent::IncomingPublish
  -> done
```

No packet-id tracking is needed.

## 5.2 QoS1

```text
incoming PUBLISH(qos1)
  -> emit RuntimeEvent::IncomingPublish
  -> send PUBACK
```

This is currently handled directly in `RuntimeTask`.

## 5.3 QoS2

```text
incoming PUBLISH(qos2)
  -> RuntimeState::on_incoming_qos2_publish_checked(pkid)
  -> if first-seen:
       send PUBREC
       emit RuntimeEvent::IncomingPublish
  -> if duplicate:
       send PUBREC
       do not emit IncomingPublish again
```

The key rule is:

- task should not guess QoS2 delivery
- state must decide whether the publish is first-seen or duplicate

Later, when broker sends `PUBREL(pkid)`, the normal driver path handles:

```text
incoming PUBREL(pkid)
  -> RuntimeState::on_incoming_pubrel_checked(pkid)
  -> send PUBCOMP(pkid)
```

So:

- `PUBREC` answers the incoming `PUBLISH`
- `PUBCOMP` answers the later `PUBREL`

## 6) Driver actions and public events

The runtime has two different output surfaces:

## 6.1 DriverAction

Internal task-facing output.

```text
Send(Packet)
Complete(Completion)
TriggerReconnect
```

These are not public API.

## 6.2 RuntimeEvent

Public client-facing output.

```text
Completion(Completion)
IncomingPublish(Publish)
Disconnected
Reconnected
```

This is what the client sees.

The translation happens in `RuntimeTask`:

```text
DriverAction::Send(packet)
  -> encode and write to transport

DriverAction::Complete(c)
  -> event_tx.send(RuntimeEvent::Completion(c))

DriverAction::TriggerReconnect
  -> event_tx.send(RuntimeEvent::Disconnected)
  -> reconnect()
```

## 7) Why the transport trait exists

`src/runtime/transport.rs`

The `Transport` trait is the boundary that lets runtime own a connection without hard-coding `TcpStream`.

Current shape:

```rust
pub trait Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
}
```

This gives you two benefits:

- `RuntimeTask` can be written against a stable interface
- tests can use a fake transport instead of a real socket

## 8) FakeTransport tests

For task-level tests, a fake transport can model the wire without opening a real socket.

Concept:

```text
reads  = chunks that the broker "will send"
writes = bytes that the runtime "has sent"
```

So tests can check:

- command publish -> encoded publish bytes show up in `writes`
- incoming ack bytes in `reads` -> completion event appears on `event_rx`
- keepalive tick -> encoded `PINGREQ` appears in `writes`

This is important because it tests:

- codec integration
- parser integration
- driver/state integration
- runtime event emission

without real networking.

## 9) Current simplifications

The current design is structurally correct, but still simplified in a few places:

- reconnect is only a hook, not a full retry/backoff loop
- the task loop is polling-based, not async/select-based
- incoming QoS1 acking happens directly in task, not yet through a richer state/driver abstraction
- transport recreation is not implemented yet

These are acceptable at this stage because the ownership model is already correct.

## 10) Mental model to keep

Use this sentence when thinking about the runtime:

```text
Commands, packets, time, and reconnects all enter through RuntimeTask.
RuntimeDriver routes them.
RuntimeState decides the legal transition.
RuntimeTask performs the real-world side effect.
```

That is the current runtime event loop in this repository.
