# Runtime QoS1 Notes

This document explains how QoS1 behavior is modeled in this repository's runtime state machine and what is still simplified compared to production MQTT clients.

## 1) Layers and responsibilities

- `Command` (`src/types.rs`): caller intent (`Publish`, `Subscribe`, etc).
- `Packet` (`src/types.rs`): wire-level MQTT packet.
- `RuntimeState` (`src/runtime/state.rs`): state transition engine that maps command intent to wire packets and broker acks back to completions.
- `PacketIdPool` (`src/runtime/pkid.rs`): non-zero packet-id candidate generation.
- `InflightStore` (`src/runtime/inflight.rs`): ack-tracking memory.

The key split is:

- codec handles `Packet <-> bytes`
- runtime handles delivery guarantees, retries, and correlation

## 2) QoS1 flow in this repo

Outgoing:

1. caller sends `Command::Publish { token_id, publish }`
2. runtime allocates `pkid` if qos > 0 and `pkid == 0`
3. runtime creates `OutgoingOp { token_id, command, packet }`
4. if offline: push to `pending`, send nothing now
5. if online: put op in `outgoing[pkid]` and return wire `Packet::Publish`

Ack:

1. broker sends `PUBACK(pkid)`
2. `on_puback_checked(pkid)` releases `outgoing[pkid]`
3. runtime returns completion `Completion::PubAck { token_id, pkid }`

Reconnect resend:

- pending QoS1 publishes are resent with `dup = true`

## 3) InflightStore fields, in practice

- `outgoing[pkid]`: active qos1/qos2 operations waiting for ack.
- `pending`: operations accepted by runtime but not sent yet (offline/backpressure/collision fallback).
- `collision`: one special deferred op blocked by an occupied `pkid` slot.
- `inflight`: current count of active `outgoing` entries.
- `last_ack`: records most recent acknowledged `pkid` (diagnostic/hook field right now).
- `outgoing_rel`: qos2 sender-side `PUBREC -> PUBREL` bookkeeping.
- `incoming_pub`: qos2 receiver-side duplicate/phase bookkeeping.

## 4) Collision model in this repo

Current design:

- `collision: Option<(u16, OutgoingOp)>` holds a single blocked op.
- when `PUBACK` frees that same `pkid`, runtime may promote that op immediately.
- if there is no collision slot, fallback is `pending`.

This is a minimal model and can be correct under light/moderate load, but it is not fully production-grade.

## 5) What is simplified vs real clients

Real MQTT clients usually maintain stronger resend/collision structures:

- multi-entry queues (often `VecDeque`) for deferred outgoing work
- deterministic fairness/order policy for replay and promotion
- full correlation between packet-id ownership and per-request lifecycle
- richer reconnect resend semantics for in-flight and unacked operations
- explicit handling for repeated collisions, not just one fast-path slot

In that context:

- single `collision` is a simplification
- `last_ack` alone is not enough for robust collision control

`last_ack` is useful for tracing and tests, but collision safety should come from the occupancy map (`outgoing`) plus deferred queues and strict transition rules.

## 6) Recommended evolution path

1. Keep `collision` as fast-path optimization if you want.
2. Treat `pending`/`deferred` queue as canonical backlog.
3. On ack release, promote from queue with deterministic policy.
4. Ensure no op is dropped when collision slot already occupied.
5. Add tests for high-collision scenarios and reconnect replay ordering.

This keeps the current architecture understandable while moving toward production behavior.
