# Runtime Driver Architecture

This document explains the role of `src/runtime/driver.rs` in the current repository.

The short version:

- `RuntimeState` owns protocol memory and state transitions.
- `RuntimeDriver` owns event dispatch and coordination.

`RuntimeDriver` is not the socket layer and not the codec. It is the bridge between external events and runtime state.

## 1) Where the driver sits

```text
App commands      Decoded broker packets      Timer ticks      Connection lifecycle
     |                     |                      |                    |
     +---------------------+----------------------+--------------------+
                                   |
                                   v
                        RuntimeDriver::handle_event(...)
                                   |
                                   v
                               RuntimeState
                                   |
                                   v
                   DriverAction::Send / Complete / TriggerReconnect
```

This means:

- `driver` receives events
- `state` decides protocol legality and mutation
- `driver` returns actions for the outer loop to execute

## 2) Core types

In `src/runtime/driver.rs`:

- `DriverEvent`
  - input to the coordinator
- `DriverAction`
  - output from the coordinator
- `RuntimeDriver`
  - thin orchestration object with:
    - `state: RuntimeState`
    - `clean_session: bool`

### DriverEvent

```text
Tick(Instant)
Command(Command)
Incoming(Packet, Instant)
ConnectionLost { clean_session }
ConnectionRestored(Instant)
```

Meaning:

- `Tick`: timer event for keepalive
- `Command`: user/runtime intent entering the protocol layer
- `Incoming`: already-decoded MQTT packet from broker
- `ConnectionLost`: transport failed or heartbeat timed out
- `ConnectionRestored`: transport reconnected

### DriverAction

```text
Send(Packet)
Complete(Completion)
TriggerReconnect
```

Meaning:

- `Send(Packet)`: outer loop should encode and write this MQTT packet
- `Complete(Completion)`: operation completed and caller can be notified
- `TriggerReconnect`: outer loop should start reconnect process

## 3) Why the driver exists

Without the driver, the outer loop would need to know:

- which `RuntimeState` method to call for each incoming packet
- how keepalive timeout maps to reconnect
- when to surface completion versus when to send another packet
- which transitions produce follow-up control packets (`PUBREL`, `PUBREC`, `PUBCOMP`)

That would spread protocol coordination across many places.

The driver centralizes this routing.

## 4) Event-by-event behavior

## 4.1 Tick

Path:

```text
Tick(now)
  -> state.on_tick(now)
     -> Some(PingReq)            => Send(PingReq)
     -> AwaitPingRespTimeout     => on_connection_lost + TriggerReconnect
     -> None                     => no action
```

Why:

- `RuntimeState` knows whether the session has been idle too long
- `RuntimeDriver` turns that decision into an action for the outer loop

## 4.2 Command

Current behavior:

- `DriverEvent::Command` is routed to `state.on_command_publish(...)`
- this currently means the driver is mainly handling `Command::Publish`

Path:

```text
Command::Publish
  -> state.on_command_publish(...)
  -> Some(Packet::Publish)  => Send(Publish)
  -> None                   => queued in pending / blocked
```

Why:

- state decides packet id allocation, inflight insertion, offline queuing, and collision handling
- driver only forwards resulting actions

## 4.3 Incoming broker packets

The driver first records incoming activity, then dispatches by packet type.

### PingResp

```text
Incoming(PingResp, now)
  -> state.note_incoming_activity(now)
  -> state.on_pingresp(now)
  -> no outgoing action
```

Effect:

- clears `await_pingresp`
- updates heartbeat timing

### PubAck

```text
Incoming(PubAck(pkid), now)
  -> state.on_puback_checked(pkid)
  -> Complete(PubAck { token_id, pkid })
  -> maybe Send(promoted collision publish)
```

Effect:

- completes outgoing QoS1
- may also unblock a deferred packet for the same packet id

### PubRec

```text
Incoming(PubRec(pkid), now)
  -> state.on_pubrec_checked(pkid)
  -> Send(PubRel(pkid))
```

Effect:

- advances outgoing QoS2 from `PUBLISH` phase to `PUBREL` phase

### PubComp

```text
Incoming(PubComp(pkid), now)
  -> state.on_pubcomp_checked(pkid)
  -> Complete(PubComp { token_id, pkid })
  -> maybe Send(promoted collision publish)
```

Effect:

- completes outgoing QoS2

### Incoming QoS2 Publish

```text
Incoming(Publish { qos2, pkid }, now)
  -> state.on_incoming_qos2_publish_checked(pkid)
  -> Send(PubRec(pkid))
```

If first seen:

- track `incoming_pub[pkid]`
- higher layer may deliver message once

If duplicate:

- do not treat it as a fresh logical delivery
- still resend `PUBREC`

### PubRel

```text
Incoming(PubRel(pkid), now)
  -> state.on_incoming_pubrel_checked(pkid)
  -> Send(PubComp(pkid))
```

Effect:

- finishes incoming QoS2 handshake

## 4.4 ConnectionLost

Path:

```text
ConnectionLost { clean_session }
  -> state.on_connection_lost(clean_session)
  -> TriggerReconnect
```

Effect:

- marks state inactive
- moves active inflight work into `pending`
- optionally clears clean-session state

## 4.5 ConnectionRestored

Path:

```text
ConnectionRestored(now)
  -> active = true
  -> reset heartbeat wait fields/timestamps
  -> state.on_connection_restored()
  -> Send(replayed pending packets)
```

Effect:

- pending outgoing work is replayed
- QoS1/QoS2 replay uses `dup = true`

## 5) Relationship with RuntimeState

Think of the split like this:

```text
RuntimeState = "What is legal? What should mutate? What packet comes next?"
RuntimeDriver = "Which state method should I call for this external event?"
```

Examples:

- `state.on_pubrec_checked(pkid)` knows that outgoing QoS2 requires `PUBREL`
- `driver.handle_event(Incoming(PubRec(...)))` knows when to call that method

The driver is deliberately thin. That is good.

## 6) Relationship with InflightStore

The driver does not directly manipulate:

- `outgoing`
- `pending`
- `collision`
- `outgoing_rel`
- `incoming_pub`

Those remain inside `RuntimeState` / `InflightStore`.

This separation matters because:

- protocol invariants stay in one place
- driver remains predictable
- tests can target:
  - state transitions alone
  - coordinator routing alone

## 7) What is still simplified

Current driver is intentionally minimal.

Notable limitations:

- `Command` path currently routes only publish logic
- there is no actual socket/eventloop implementation here yet
- `DriverAction::Complete` is surfaced, but upper-layer completion routing is still up to future runtime code
- reconnect backoff/jitter/policy is not implemented here

So this driver is a pure coordinator, not a full transport runtime.

## 8) Test coverage

Driver behavior is currently covered by:

- `tests/runtime_driver.rs`
  - timeout reconnect trigger
  - completion emission for `PUBACK`
  - completion emission for `PUBCOMP`
- `tests/runtime_driver_matrix.rs`
  - tick idle/inactive behavior
  - connection lost / restored behavior
  - incoming `PINGRESP`
  - QoS2 incoming/outgoing routing
  - collision promotion through driver
  - unsolicited ack propagation

## 9) Mental model

If you want one sentence to remember:

```text
RuntimeState is the protocol brain; RuntimeDriver is the traffic controller.
```

Or as a shorter loop:

```text
Event in -> state transition -> actions out
```
