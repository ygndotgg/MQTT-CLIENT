# Runtime QoS2 Architecture

This document describes how QoS2 is handled in this repository's runtime state machine.

## 1) Big picture

QoS2 is not one state machine. It is two:

1. Outgoing QoS2 (client is sender)
2. Incoming QoS2 (client is receiver)

Both use the same packet id (`pkid`) namespace, but track different phases.

```text
Outgoing (sender side):
PUBLISH -> PUBREC -> PUBREL -> PUBCOMP -> complete

Incoming (receiver side):
PUBLISH -> PUBREC -> PUBREL -> PUBCOMP -> done
```

## 2) Runtime structures and their roles

In `src/runtime/state.rs` and `src/runtime/inflight.rs`:

- `inflight.outgoing[pkid] = Option<OutgoingOp>`
  - Tracks active outgoing QoS1/QoS2 operations waiting for terminal ack.
- `inflight.outgoing_rel[pkid] = bool`
  - Outgoing QoS2 phase marker: `PUBREC` seen and `PUBREL` phase active.
- `inflight.incoming_pub[pkid] = bool`
  - Incoming QoS2 duplicate suppression marker: incoming QoS2 publish has been recorded and is waiting for `PUBREL`.
- `Completion::PubComp { token_id, pkid }`
  - App-facing completion when outgoing QoS2 reaches terminal `PUBCOMP`.

## 3) Outgoing QoS2 flow

```text
Command::Publish(qos2)
   |
   | on_command_publish
   v
outgoing[pkid] = Some(op), outgoing_rel[pkid] = false
send PUBLISH(pkid)
   |
   | on_pubrec_checked(pkid)
   v
outgoing_rel[pkid] = true
send PUBREL(pkid)
   |
   | on_pubcomp_checked(pkid)
   v
clear outgoing_rel[pkid]
release outgoing[pkid]
emit Completion::PubComp { token_id, pkid }
```

Validation points:

- `on_pubrec_checked` rejects unsolicited `PUBREC` if `outgoing[pkid]` is empty.
- `on_pubcomp_checked` rejects unsolicited `PUBCOMP` unless:
  - `outgoing_rel[pkid] == true`, and
  - `outgoing[pkid]` still exists.

## 4) Incoming QoS2 flow

```text
incoming PUBLISH(qos2, pkid)
   |
   | on_incoming_qos2_publish_checked(pkid)
   v
if first-seen:
  incoming_pub[pkid] = true
  return FirstSeen + PUBREC(pkid)
if duplicate:
  return Duplicate + PUBREC(pkid)   (no redelivery)
   |
   | on_incoming_pubrel_checked(pkid)
   v
require incoming_pub[pkid] == true
clear incoming_pub[pkid]
send PUBCOMP(pkid)
```

Duplicate suppression behavior:

- first publish: tracked and acked
- repeated publish with same `pkid` before `PUBREL`: not treated as new delivery

## 5) Collision interaction

QoS2 shares the same collision promotion mechanism as QoS1:

- If a slot collision exists for a `pkid`, promotion can happen when that `pkid` is released.
- For QoS2, release happens at `PUBCOMP` (terminal ack), not at `PUBREC`.

```text
collision(pkid=X) blocked
...
on_pubcomp_checked(X) releases slot X
-> promote_collision(X)
-> next packet may be returned for immediate send
```

## 6) Error model

Current typed errors used by QoS2 path:

- `UnsolicitedPubRec(pkid)`
- `UnsolicitedPubComp(pkid)`
- `UnsolicitedPubRel(pkid)`
- `PacketIdOutOfRange(pkid)`

This prevents panics on malformed or out-of-order control flow.

## 7) Test coverage map

`tests/runtime_qos2.rs` validates:

- outgoing happy path (`PUBLISH -> PUBREC -> PUBREL -> PUBCOMP`)
- unsolicited `PUBREC` and `PUBCOMP`
- incoming duplicate suppression
- unsolicited incoming `PUBREL`
- collision promotion on `PUBCOMP`

This gives a practical correctness baseline for Day 10.
