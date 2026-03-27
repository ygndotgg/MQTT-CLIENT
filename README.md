# mqtt-client

A from-scratch MQTT 3.1.1 client runtime in Rust.

This repository is not a wrapper around an existing client. It is a learning-first implementation of the pieces that actually make an MQTT client work over time:

- strict protocol codec
- protocol state machine
- inflight ownership
- keepalive and reconnect preparation
- single event loop runtime
- multi logical client routing over one runtime

The protocol codec is derived from the MQTT 3.1.1 specification.
The runtime ownership model is inspired by `rumqttc`, but the code here is intentionally smaller and more explicit so the architecture stays easy to study.

## Current Status

Implemented today:

- strict MQTT 3.1.1 packet encoding and decoding
- fixed-header and remaining-length parsing
- stream fragmentation handling
- runtime state for QoS1 and QoS2
- packet id allocation and inflight tracking
- pending replay preparation on reconnect
- keepalive tick handling with `PINGREQ` / `PINGRESP`
- multi-client API layer with:
  - `RuntimeHost`
  - per-client `ClientHandle`
  - targeted completion routing
  - subscription-based incoming publish fanout
- integration tests for reconnect, replay, subscribe/unsubscribe, and multi-client routing

Important scope note:

- this is a runtime core and learning project first
- transport is abstracted behind `runtime::transport::Transport`
- there is no polished `TcpStream` / TLS / broker bootstrap layer in the public API yet

That means the architecture is real, but the network integration surface is intentionally still minimal.

## Why This Project Exists

Most MQTT explanations stop at packet shapes.
That is not where the difficult part starts.

The difficult part starts when packets stop being isolated:

- a publish is not complete when bytes leave the process
- QoS requires memory
- reconnect requires replay
- incoming QoS2 requires duplicate suppression
- one runtime may need to serve many logical callers

This project exists to make those ownership boundaries explicit.

## Architecture At A Glance

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

Ownership by layer:

- `ClientHandle`
  - expresses user intent
  - receives per-client runtime events
- `RuntimeHost`
  - starts one runtime
  - creates logical clients later
- `RuntimeTask`
  - owns the live event loop
  - owns transport, parser, reconnect orchestration, and library-side routing
- `RuntimeDriver`
  - dispatches external events into protocol transitions
- `RuntimeState`
  - owns MQTT truth over time
- `InflightStore`
  - remembers unfinished operations
- `ClientRegistry`
  - `client_id -> mailbox`
- `Router`
  - `topic filter -> client_ids`

## What Is Implemented

### Protocol Codec

Implemented packet families:

- `CONNECT`
- `CONNACK`
- `PUBLISH`
- `PUBACK`
- `PUBREC`
- `PUBREL`
- `PUBCOMP`
- `SUBSCRIBE`
- `SUBACK`
- `UNSUBSCRIBE`
- `UNSUBACK`
- `PINGREQ`
- `PINGRESP`
- `DISCONNECT`

Codec properties:

- strict validation of fixed-header flags
- strict remaining-length parsing
- MQTT string and bytes primitive coverage
- packet-type specific validation
- fragmented stream support via `codec::stream::FrameParser`

### Runtime Core

Implemented runtime behaviors:

- outgoing QoS1 publish tracking
- outgoing QoS2 publish phase tracking
- incoming QoS2 duplicate suppression
- completion emission on terminal acks
- collision handling for packet id slots
- keepalive timeout detection
- reconnect trigger path
- pending replay reconstruction
- DUP marking on replayed QoS1 / QoS2 publish

### Multi-Client Library Layer

Implemented library behaviors:

- many logical `ClientHandle`s over one runtime
- shared command lane
- isolated event receivers per client
- targeted completion delivery using stored `client_id`
- subscription-driven incoming publish fanout
- route add on `SUBACK`
- route remove on `UNSUBACK`
- disconnect / reconnect broadcast to all clients

## What Is Deliberately Not Finished Yet

This repository does not currently try to hide its unfinished edges.

Still intentionally thin:

- real socket bootstrap and reconnect dialing
- a production `TcpStream` transport adapter
- TLS
- async runtime integration
- manual-ack policy modes
- advanced router policies beyond broadcast-style fanout
- production-grade backpressure and batching machinery like `rumqttc`'s `xchg`

If your goal is to learn how an MQTT client works internally, that is an advantage.
If your goal is to drop in a finished broker client, this repository is not there yet.

## Public Surface

Current top-level modules:

```text
src/
  client.rs
  codec/
  protocol/
  runtime/
  types.rs
```

Current runtime modules:

```text
src/runtime/
  driver.rs
  events.rs
  host.rs
  inflight.rs
  pkid.rs
  router.rs
  state.rs
  task.rs
  transport.rs
```

Minimal public runtime shape:

```rust
pub struct ClientHandle { /* client_id, command_tx, event_rx */ }
pub struct RuntimeHost { /* starts one runtime, creates clients */ }

pub trait Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
}
```

## How To Read The Codebase

Recommended reading order:

1. [`src/types.rs`](src/types.rs)
2. [`src/codec/`](src/codec)
3. [`src/runtime/inflight.rs`](src/runtime/inflight.rs)
4. [`src/runtime/state.rs`](src/runtime/state.rs)
5. [`src/runtime/driver.rs`](src/runtime/driver.rs)
6. [`src/runtime/task.rs`](src/runtime/task.rs)
7. [`src/runtime/host.rs`](src/runtime/host.rs)
8. [`src/runtime/router.rs`](src/runtime/router.rs)
9. [`tests/`](tests)

That order follows the same progression the project itself took:

- codec
- protocol memory
- runtime state transitions
- event loop ownership
- multi-client routing

## Test Coverage

The repository leans heavily on tests.

Coverage areas include:

- primitives and remaining length
- fixed header parsing
- connect / connack
- publish and ack families
- subscribe / unsubscribe families
- stream fragmentation
- runtime QoS1
- runtime QoS2
- keepalive and reconnect
- driver action matrix
- multi-client routing and replay

Useful commands:

```bash
cargo test --lib
cargo test --tests
cargo test --test multi_client_runtime -- --nocapture
```

## Documentation

Runtime docs:

- [docs/runtime/README.md](docs/runtime/README.md)
- [docs/runtime/architecture.md](docs/runtime/architecture.md)
- [docs/runtime/event-loop.md](docs/runtime/event-loop.md)
- [docs/runtime/driver.md](docs/runtime/driver.md)
- [docs/runtime/function-reference.md](docs/runtime/function-reference.md)
- [docs/runtime/qos1.md](docs/runtime/qos1.md)

Long-form build notes / article draft:

- [docs/blog/mqtt-client-from-scratch-step-1.md](docs/blog/mqtt-client-from-scratch-step-1.md)

## Relation To `rumqttc`

This project was built partly to understand how `rumqttc` works.

The mapping is roughly:

- `RuntimeTask` ~= `rumqttc` event loop + part of connection ownership
- `RuntimeState` ~= `rumqttc` MQTT state
- `OutgoingOp` ~= stored outgoing ownership used for ack routing
- `ClientRegistry` + `Router` ~= the multi-client library routing layer

The important difference is that this repository is intentionally flatter and easier to inspect.
It keeps fewer moving parts so the ownership model stays visible.

## Summary

This project is best understood as:

```text
a strict MQTT 3.1.1 codec
+ a stateful runtime
+ reconnect-aware protocol memory
+ a multi-client library layer
```

If you want to study how an MQTT client actually behaves over time, start here.
