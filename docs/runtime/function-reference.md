# Runtime Function Reference

This document is a function-by-function map of the current runtime and multi-client library surface.

It explains:

- what each function is responsible for
- what it receives
- what it returns
- what state it mutates
- how it fits into the end-to-end runtime flow

This is not a protocol specification. It is a code-level reference for the current architecture in this repository.

## 1) Current runtime picture

The current runtime is split into these layers:

```text
ClientHandle
   |
   | sends Command
   v
RuntimeHost
   |
   | starts one RuntimeTask and creates logical clients
   v
RuntimeTask
   |
   | owns transport, parser, router, registry, timer polling
   v
RuntimeDriver
   |
   | dispatches DriverEvent
   v
RuntimeState
   |
   | owns MQTT protocol truth
   v
InflightStore / PacketIdPool
```

The most important rule is:

- `ClientHandle` is thin
- `RuntimeHost` bootstraps and allocates logical clients
- `RuntimeTask` owns runtime execution and library-side routing
- `RuntimeDriver` routes external events into state
- `RuntimeState` owns protocol legality and replay state

## 2) `src/client.rs`

`ClientHandle` is the user-facing handle for one logical client.

It owns:

- `client_id`
- shared `command_tx`
- per-client `event_rx`
- local token counter

### `ClientHandle::new(client_id, command_tx, event_rx) -> Self`

Purpose:
- build one logical client handle

Input:
- `client_id`: logical identity assigned by `RuntimeHost`
- `command_tx`: shared sender into the one runtime task
- `event_rx`: receiver dedicated to this logical client

Output:
- a `ClientHandle`

Why it exists:
- each client needs its own event mailbox
- all clients still share one runtime command lane

### `ClientHandle::next_token_id(&mut self) -> usize`

Purpose:
- allocate the next client-local token id

Input:
- mutable self

Output:
- monotonically increasing token id

Side effect:
- increments the local counter

Why it exists:
- outgoing commands need a caller-visible completion token

### `ClientHandle::publish(...) -> Result<usize, SendError<Command>>`

Purpose:
- turn a publish request from user code into a runtime `Command`

Input:
- topic
- qos
- retain
- payload

Output:
- `Ok(token_id)` if the command was sent
- `Err(...)` if the runtime command channel is closed

What it does:
1. allocates `token_id`
2. builds `Publish`
3. sends `Command::Publish { client_id, token_id, publish }`

Why it exists:
- the client API should express intent, not wire packets

### `ClientHandle::subscribe(...) -> Result<usize, SendError<Command>>`

Purpose:
- turn a subscribe request into a runtime `Command`

What it does:
1. allocates `token_id`
2. builds `Subscribe`
3. sends `Command::Subscribe { client_id, token_id, subscribe }`

Why it exists:
- subscription ownership must be attached to the logical client that requested it

### `ClientHandle::unsubscribe(...) -> Result<usize, SendError<Command>>`

Purpose:
- turn an unsubscribe request into a runtime `Command`

What it does:
1. allocates `token_id`
2. builds `Unsubscribe`
3. sends `Command::Unsubscribe { client_id, token_id, unsubscribe }`

Why it exists:
- unsubscribe completion and later route removal must be attributed to the correct logical client

### `ClientHandle::recv_event(&self) -> Result<RuntimeEvent, RecvError>`

Purpose:
- let caller wait for the next event for this logical client

Output:
- one `RuntimeEvent`

Why it exists:
- the client handle receives completions and incoming publishes through its own channel

## 3) `src/runtime/host.rs`

`RuntimeHost` is the multi-client bootstrap and client factory.

It owns:

- shared command sender
- shared registry of logical-client event mailboxes

### `RuntimeHost::new(transport, max_inflight, clean_session) -> Self`

Purpose:
- create the runtime system once

Input:
- transport
- max inflight size
- clean session flag

Output:
- a `RuntimeHost`

What it does:
1. creates shared command channel
2. creates shared `ClientRegistry`
3. builds `RuntimeState`
4. builds `RuntimeDriver`
5. builds `RuntimeTask`
6. spawns the runtime thread

Why it exists:
- runtime starts once, but clients may be created later

### `RuntimeHost::new_client(&self) -> ClientHandle`

Purpose:
- create one more logical client attached to the same runtime

Output:
- a new `ClientHandle`

What it does:
1. creates a new per-client event channel
2. registers its sender into `ClientRegistry`
3. gets a fresh `client_id`
4. returns `ClientHandle { client_id, shared command_tx, unique event_rx }`

Why it exists:
- commands are shared
- events are per-client

## 4) `src/runtime/task.rs`

`RuntimeTask` is the actual running event loop owner.

It owns:

- `command_rx`
- shared `ClientRegistry`
- `RuntimeDriver`
- transport
- `FrameParser`
- tick scheduling fields
- `Router`

### `ClientRegistry::register(event_tx) -> usize`

Purpose:
- register one logical client's event mailbox

Output:
- new `client_id`

What it does:
- increments `next_id`
- stores `client_id -> Sender<RuntimeEvent>`

Why it exists:
- runtime needs a central address book for logical clients

### `ClientRegistry::remove(client_id) -> Option<Sender<RuntimeEvent>>`

Purpose:
- remove a client mailbox from the registry

Output:
- removed sender if present

Why it exists:
- logical clients may disappear while runtime continues running

### `ClientRegistry::send_to(client_id, event) -> Result<(), SendError<_>>`

Purpose:
- route one event to one logical client

What it does:
- looks up the sender by `client_id`
- sends the event if the sender exists

Current behavior:
- missing client id is treated as `Ok(())`

Why it exists:
- completions and fanout deliveries are client-specific

### `ClientRegistry::broadcast(event)`

Purpose:
- route one shared event to all registered logical clients

What it does:
- clones `RuntimeEvent`
- sends it to each registered client sender

Why it exists:
- disconnect/reconnect are connection-wide library events

### `RuntimeTask::new(command_rx, registry, driver, transport, tick_interval) -> Self`

Purpose:
- construct the runtime task state before spawning the thread

What it initializes:
- command receiver
- registry
- driver
- parser
- transport
- tick interval
- last tick timestamp
- router

### `RuntimeTask::fanout_publish(&self, publish) -> Result<(), RuntimeTaskError>`

Purpose:
- route one incoming application publish to the logical clients whose filters match the topic

What it does:
1. asks `router.matching_clients(&publish.topic)` for recipient client ids
2. clones the publish for each matching client
3. sends `RuntimeEvent::IncomingPublish(publish)` through `send_event_to(...)`

Why it exists:
- runtime task owns library-side message fanout
- protocol state should not decide which logical client receives a publish

Important consequence:
- no matching clients means the publish is accepted at the protocol level but delivered to no local client handle

### `RuntimeTask::send_event_to(client_id, event) -> Result<(), RuntimeTaskError>`

Purpose:
- deliver one event to one logical client through the registry

What it does:
- locks the registry
- calls `registry.send_to(...)`
- maps channel failure to `RuntimeTaskError::EventChannelClosed`

Why it exists:
- task should express targeted event delivery explicitly

### `RuntimeTask::broadcast_event(event)`

Purpose:
- deliver one event to all registered clients

Why it exists:
- disconnect/reconnect are not tied to one logical client

### `RuntimeTask::send_completion_to(client_id, completion) -> Result<(), RuntimeTaskError>`

Purpose:
- convenience wrapper for routing one completion to the owning client

What it does:
- wraps `Completion` as `RuntimeEvent::Completion`
- calls `send_event_to(...)`

Why it exists:
- completion routing is client-aware, but `Completion` itself stays protocol-facing

### `RuntimeTask::read_one_packet(&mut self) -> Result<Option<Packet>, RuntimeTaskError>`

Purpose:
- read bytes from transport and convert them into at most one decoded `Packet`

Input:
- transport byte stream

Output:
- `Ok(None)` when no full MQTT frame is ready yet
- `Ok(Some(packet))` when one full packet is decoded
- `Err(...)` on I/O or protocol decoding failure

What it does:
1. reads bytes from transport
2. pushes them into `FrameParser`
3. asks parser for the next packet

Why it exists:
- transport is a byte stream, but runtime logic works on whole MQTT packets

### `RuntimeTask::recv_cmd(&mut self) -> Result<Command, RuntimeTaskError>`

Purpose:
- blocking receive of one command

Current use:
- used by `run_once()` in tests and simple single-command flows

Why it exists:
- useful for testing and for the initial command-only task phase

### `RuntimeTask::run_once(&mut self) -> Result<(), RuntimeTaskError>`

Purpose:
- process one blocking command from the command channel

What it does:
1. waits for one command
2. converts it into `DriverEvent::Command`
3. passes it to the driver
4. executes resulting actions

Why it exists:
- simpler test hook before relying only on polling loop behavior

### `RuntimeTask::handle_incoming_publish(&mut self, publish, now) -> Result<(), RuntimeTaskError>`

Purpose:
- handle incoming broker `PUBLISH` packets specially

Why it is special:
- incoming publish is both:
  - a protocol event
  - an application-facing event

Common behavior:
- updates `last_incoming` via `state.note_incoming_activity(now)`
- then branches by QoS

Current behavior by QoS:

- QoS0:
  - route through `fanout_publish(...)`

- QoS1:
  - route through `fanout_publish(...)`
  - send `PUBACK`

- QoS2:
  - call `state.on_incoming_qos2_publish_checked(pkid)`
  - on first seen:
    - send `PUBREC`
    - route through `fanout_publish(...)`
  - on duplicate:
    - send `PUBREC`
    - do not fan out again

Why this matters:
- application delivery and protocol acknowledgment are related but not identical
- QoS2 duplicate suppression must happen before publish fanout

### `RuntimeTask::reconnect(&mut self) -> Result<(), RuntimeTaskError>`

Purpose:
- restore runtime state after reconnect trigger

Current simplified behavior:
1. emits `DriverEvent::ConnectionRestored(now)` into the driver
2. executes replay/send actions returned by the driver
3. broadcasts `RuntimeEvent::Reconnected`

Why it exists:
- task owns reconnect orchestration, not `RuntimeState`

Important limitation:
- current implementation does not recreate a real transport connection yet
- it restores protocol state and replay path only

### `RuntimeTask::poll_incoming(&mut self) -> Result<(), RuntimeTaskError>`

Purpose:
- non-blocking incoming network side of the event loop

What it does:
1. tries to decode one packet with `read_one_packet()`
2. if no packet, returns `Ok(())`
3. if packet is `Publish`, delegates to `handle_incoming_publish(...)`
4. otherwise routes packet into the driver as `DriverEvent::Incoming`
5. executes returned actions

Why it exists:
- runtime must keep network progress moving even when no command arrives

### `RuntimeTask::poll_command(&mut self) -> Result<(), RuntimeTaskError>`

Purpose:
- non-blocking command side of the event loop

What it does:
1. calls `try_recv()` on the command channel
2. if a command exists:
   - routes it through the driver
   - executes resulting actions
3. if channel empty:
   - returns `Ok(())`
4. if channel disconnected:
   - returns `CommandChannelClosed`

Why it exists:
- runtime must also process transport and ticks
- blocking `recv()` here would freeze those other responsibilities

### `RuntimeTask::poll_tick(&mut self) -> Result<(), RuntimeTaskError>`

Purpose:
- periodic time-based side of the event loop

What it does:
1. checks whether `tick_interval` has elapsed since the last tick
2. if not, returns `Ok(())`
3. if yes:
   - updates `last_tick`
   - routes `DriverEvent::Tick(now)` through the driver
   - executes returned actions

Why it exists:
- task owns the time source
- driver/state own keepalive semantics

### `RuntimeTask::handle_actions(&mut self, actions) -> Result<(), RuntimeTaskError>`

Purpose:
- execute the side effects returned by the driver

Current behavior:

- `DriverAction::Send(packet)`
  - encode and write packet to transport

- `DriverAction::CompleteFor { client_id, completion }`
  - send completion only to the owning client

- `DriverAction::TriggerReconnect`
  - broadcast `Disconnected`
  - run reconnect flow

- `DriverAction::SubscribeAckFor { client_id, filters, completion }`
  - add each acknowledged filter to the router for that client
  - then send completion to the owning client

- `DriverAction::UnsubscribeAckFor { client_id, filters, completion }`
  - remove each acknowledged filter from the router for that client
  - then send completion to the owning client

Why it exists:
- driver decides what should happen
- task performs the actual side effects
- router mutation is a library concern, so it happens here, not inside `RuntimeState`

### `RuntimeTask::apply_actions_for_test(&mut self, actions) -> Result<(), RuntimeTaskError>`

Purpose:
- test hook for directly applying `DriverAction`s

Why it exists:
- allows unit tests to validate task-side action handling without a full runtime loop

### `RuntimeTask::run(self) -> Result<(), RuntimeTaskError>`

Purpose:
- main event loop

Current loop:

```text
loop {
  poll_command()?;
  poll_incoming()?;
  poll_tick()?;
}
```

Why it exists:
- runtime task is the single owner of command, network, time, and routing progress

### `RuntimeTask::send_packet(&mut self, packet) -> Result<(), RuntimeTaskError>`

Purpose:
- turn one `Packet` into wire bytes and write them to transport

What it does:
1. encodes `Packet` into bytes
2. writes bytes to transport
3. updates outgoing activity timestamp

Why it exists:
- task owns the real write side of the connection

## 5) `src/runtime/router.rs`

`Router` is library-side publish fanout state. It is not MQTT session state.

It owns:

- a list of subscription filters
- the logical clients interested in each filter

### `Router::add_subscription(client_id, filter)`

Purpose:
- remember that one logical client is interested in one subscription filter

What it does:
- finds an existing entry for the filter or creates one
- deduplicates client ids within that entry

Why it exists:
- incoming publish fanout should be driven by successful subscribe ownership

### `Router::remove_subscription(client_id, filter)`

Purpose:
- stop delivering a filter to one logical client

What it does:
- removes the client id from the filter entry
- removes the filter entry entirely if no clients remain

Why it exists:
- route removal must happen when unsubscribe is acknowledged

### `Router::matching_clients(topic) -> Vec<usize>`

Purpose:
- find the logical clients whose stored filters match an incoming publish topic

What it does:
- scans entries
- uses `topic_matches(...)`
- returns deduplicated client ids

Why it exists:
- `RuntimeTask` needs a pure routing answer before sending `IncomingPublish` events

### `Router::topic_matches(filter, topic) -> bool`

Purpose:
- evaluate MQTT topic filter matching for `+`, `#`, and exact levels

Current supported semantics:
- exact level match
- `+` for one level
- `#` for remaining levels at the end of the filter

Why it exists:
- router fanout is filter-based, not exact-string-based

## 6) `src/runtime/driver.rs`

`RuntimeDriver` converts external events into protocol transitions plus task actions.

### `RuntimeDriver::new(state, clean_session) -> Self`

Purpose:
- construct the coordinator around a `RuntimeState`

### `RuntimeDriver::handle_event(event) -> Result<Vec<DriverAction>, RuntimeError>`

Purpose:
- central event dispatch entrypoint

Input:
- `DriverEvent`

Output:
- one or more `DriverAction`s

This is the function that decides which `RuntimeState` method to call for:

- commands
- incoming packets
- ticks
- reconnect lifecycle

### `DriverAction::Send(Packet)`

Meaning:
- task should encode and write this packet

### `DriverAction::TriggerReconnect`

Meaning:
- task should enter reconnect flow and broadcast connection loss

### `DriverAction::CompleteFor { client_id, completion }`

Meaning:
- completion belongs to exactly one logical client
- task should route it with `registry.send_to(client_id, ...)`

Why this action exists:
- `Completion` alone is protocol information
- `client_id` is library routing information

### `DriverAction::SubscribeAckFor { client_id, filters, completion }`

Meaning:
- broker acknowledged a subscribe owned by `client_id`
- task should install those filters into the router
- task should then send the completion to the owning client

Why this action exists:
- subscribe route installation is a library-side effect, not protocol truth

### `DriverAction::UnsubscribeAckFor { client_id, filters, completion }`

Meaning:
- broker acknowledged an unsubscribe owned by `client_id`
- task should remove those filters from the router
- task should then send the completion to the owning client

Why this action exists:
- unsubscribe route removal must be delayed until ack, including after reconnect replay

## 7) `src/runtime/state.rs`

`RuntimeState` owns protocol legality and long-lived MQTT state.

### `RuntimeState::new(max_inflight) -> Self`

Purpose:
- create state with default keepalive

Current default:
- 30 seconds

### `RuntimeState::new_with_keep_alive(max_inflight, keep_alive) -> Self`

Purpose:
- create state with explicit keepalive

### `RuntimeState::on_suback_checked(pkid) -> Result<SubscribeAckResult, RuntimeError>`

Purpose:
- process a broker `SUBACK`

Output:
- `SubscribeAckResult { client_id, token_id, filters, completion }`

What it does:
1. releases the matching outgoing subscribe op from inflight
2. extracts the owning logical client id and subscribed filters
3. builds `Completion::SubAck`

Why it exists:
- the runtime must know which logical client owns the subscribe and which filters should be installed into the router

### `RuntimeState::on_unsuback_checked(pkid) -> Result<UnsubscribeAckResult, RuntimeError>`

Purpose:
- process a broker `UNSUBACK`

Output:
- `UnsubscribeAckResult { client_id, token_id, filters, completion }`

What it does:
1. releases the matching outgoing unsubscribe op from inflight
2. extracts the owning logical client id and unsubscribed filters
3. builds `Completion::UnsubAck`

Why it exists:
- route removal must happen only after the broker acknowledges the unsubscribe

### `RuntimeState::on_tick(now) -> Result<Option<Packet>, RuntimeError>`

Purpose:
- run keepalive logic

Output:
- `Ok(Some(Packet::PingReq))` when a ping must be sent
- `Ok(None)` when no action is needed
- `Err(AwaitPingRespTimeout)` when reconnect is required

What it checks:
- active/inactive status
- missing `PINGRESP`
- idle duration since last incoming/outgoing activity

### `RuntimeState::on_pingresp(now)`

Purpose:
- finish the ping wait phase

What it does:
- clears `await_pingresp`
- updates `last_incoming`

### `RuntimeState::on_connection_lost(clean_session)`

Purpose:
- cleanup state after disconnect

What it does:
- marks inactive
- clears ping wait
- moves active inflight and collision ops into pending
- resets counters
- optionally clears session-dependent tracking on clean session

Important consequence:
- outstanding publish, subscribe, and unsubscribe ops become reconnect-replay candidates

### `RuntimeState::on_connection_restored() -> Vec<Packet>`

Purpose:
- restore runtime to active state and produce replay packets

What it does:
- marks active
- calls `resend_pending_on_reconnect()`

### `RuntimeState::note_incoming_activity(now)`

Purpose:
- record that a packet was received

### `RuntimeState::note_outgoing_activity(now)`

Purpose:
- record that a packet was sent

### `RuntimeState::set_active(active)`

Purpose:
- update coarse connection state

### `RuntimeState::on_pubrec_checked(pkid) -> Result<Packet, RuntimeError>`

Purpose:
- process outgoing QoS2 `PUBREC`

Output:
- `Packet::PubRel`

Why:
- advances outgoing QoS2 sender state

### `RuntimeState::on_pubcomp_checked(pkid) -> Result<AckResult, RuntimeError>`

Purpose:
- process outgoing QoS2 terminal completion

Output:
- `AckResult { client_id, completion, next_packet }`

What it does:
- validates phase
- clears outgoing QoS2 `PUBREL` state
- releases inflight op
- returns owning client id
- may promote a collision packet

### `RuntimeState::on_incoming_qos2_publish_checked(pkid) -> Result<IncomingQos2Result, RuntimeError>`

Purpose:
- process incoming QoS2 duplicate suppression

Output:
- `FirstSeen { pubrec }`
- `Duplicate { pubrec }`

Why:
- incoming QoS2 delivery should only happen once

### `RuntimeState::on_incoming_pubrel_checked(pkid) -> Result<Packet, RuntimeError>`

Purpose:
- finish incoming QoS2 receiver-side state

Output:
- `Packet::PubComp`

### `RuntimeState::idx(pkid) -> Result<usize, RuntimeError>`

Purpose:
- internal helper to validate packet id and convert to index

Why:
- packet id `0` is invalid for tracked inflight operations

### `RuntimeState::clean_for_reconnect(clean_session)`

Purpose:
- compatibility wrapper around `on_connection_lost(...)`

### `RuntimeState::on_command_subscribe(command) -> Result<Option<Packet>, RuntimeError>`

Purpose:
- process an outgoing subscribe command

Input:
- `Command::Subscribe { client_id, token_id, subscribe }`

Output:
- `Some(Packet::Subscribe)` when packet should be sent now
- `None` when packet is queued/deferred

What it does:
1. extracts `client_id`, `token_id`, and subscribe body
2. allocates packet id if needed
3. builds `OutgoingOp`
4. if inactive:
   - moves op to pending
5. if active:
   - inserts op into inflight or pushes to pending on collision/full conditions

Why it exists:
- subscribe replay and later route installation need the original ownership information preserved in inflight

### `RuntimeState::on_command_unsubscribe(command) -> Result<Option<Packet>, RuntimeError>`

Purpose:
- process an outgoing unsubscribe command

Input:
- `Command::Unsubscribe { client_id, token_id, unsubscribe }`

Output:
- `Some(Packet::UnSubscribe)` when packet should be sent now
- `None` when packet is queued/deferred

What it does:
1. extracts `client_id`, `token_id`, and unsubscribe body
2. allocates packet id if needed
3. builds `OutgoingOp`
4. if inactive:
   - moves op to pending
5. if active:
   - inserts op into inflight or pushes to pending on collision/full conditions

Why it exists:
- unsubscribe replay and delayed route removal both depend on retaining the original logical-client ownership

### `RuntimeState::on_command_publish(command) -> Result<Option<Packet>, RuntimeError>`

Purpose:
- process an outgoing publish command

Input:
- `Command::Publish { client_id, token_id, publish }`

Output:
- `Some(Packet::Publish)` when packet should be sent now
- `None` when packet is queued/deferred

What it does:
1. extracts `client_id`, `token_id`, and publish
2. allocates packet id if QoS requires it
3. builds `OutgoingOp`
4. if inactive:
   - moves op to pending
5. if active and QoS requires tracking:
   - inserts into inflight
   - may fall back to pending on collision/full conditions

Why it exists:
- this is the core outgoing publish state transition

### `RuntimeState::resend_pending_on_reconnect() -> Vec<Packet>`

Purpose:
- replay pending operations after reconnect

What it does:
- drains pending
- rebuilds tracked publish operations
- rebuilds tracked subscribe operations
- rebuilds tracked unsubscribe operations
- marks QoS1/QoS2 publish resends with `dup = true`
- re-inserts inflight state where needed
- returns packets to send

Why it exists:
- reconnect replay is protocol state work, not host/client work

Important consequence:
- subscribe routes are not reinstalled here
- unsubscribe routes are not removed here
- router changes still wait for `SUBACK` / `UNSUBACK`

### `RuntimeState::on_puback_checked(pkid) -> Result<AckResult, RuntimeError>`

Purpose:
- process outgoing QoS1 completion

Output:
- `AckResult { client_id, completion, next_packet }`

What it does:
- releases inflight op
- returns owning client id
- may promote collision packet

### `RuntimeState::promote_collision(acked_pkid) -> Option<Packet>`

Purpose:
- unblock one deferred collision packet when the matching packet-id slot is freed

What it does:
- checks whether stored collision matches the just-acked packet id
- marks replayed publish as `dup`
- re-inserts it into inflight
- returns wire packet if promotion succeeds

### `mark_publish_dup(packet) -> Packet`

Purpose:
- set `dup = true` on replayed QoS1/QoS2 publishes

Why:
- resend semantics must be visible on the wire

## 8) `src/runtime/inflight.rs`

`InflightStore` is the runtime's packet-tracking memory.

It owns:

- active outgoing ops indexed by packet id
- pending deferred ops
- one collision slot
- outgoing QoS2 `PUBREL` tracking
- incoming QoS2 duplicate suppression tracking

### `OutgoingOp`

Purpose:
- store the long-lived ownership and replay information for one outgoing operation

Fields that matter for multi-client runtime:
- `token_id`: completion identity within the owning client
- `client_id`: logical client that owns this operation
- `command`: original command for replay and ack interpretation
- `packet`: last built wire packet

Why it exists:
- targeted completion routing and reconnect replay both depend on retaining the original logical-client ownership

## 9) Integration-tested runtime flows

The integration tests in `tests/multi_client_runtime.rs` act as executable documentation for the current multi-client runtime behavior.

### 9.1 Subscribe installs routes and incoming publish fans out by topic

Flow:
1. client A sends `Subscribe(sensor/+)`
2. client B sends `Subscribe(alerts/#)`
3. runtime writes two `SUBSCRIBE` packets
4. broker sends matching `SUBACK`s
5. driver emits `SubscribeAckFor` for each owning client
6. task adds filters into the router
7. task routes `Completion::SubAck` only to the owning client
8. later incoming publish on `sensor/temp` is fanned out only to client A

### 9.2 Publish completion is targeted to the originating client

Flow:
1. client A sends QoS1 publish on `cmd/out`
2. runtime stores `OutgoingOp { client_id: A, token_id, ... }`
3. broker sends `PUBACK(pkid)`
4. state releases the inflight op and returns `AckResult { client_id: A, ... }`
5. driver emits `CompleteFor { client_id: A, completion }`
6. task sends `RuntimeEvent::Completion(...)` only to client A

### 9.3 Reconnect broadcasts lifecycle events and replays pending publish

Flow:
1. a tracked QoS1 publish is sent
2. keepalive timeout triggers `TriggerReconnect`
3. task broadcasts `Disconnected` to all logical clients
4. task emits `ConnectionRestored` into the driver
5. state replays pending work
6. replayed publish is written again with `dup = true`
7. task broadcasts `Reconnected` to all logical clients
8. broker `PUBACK` completes only the owning client
9. router-backed publish fanout still works after reconnect

### 9.4 Reconnect replays unsubscribe, but route removal waits for `UNSUBACK`

Flow:
1. both clients subscribe to `sensor/+`
2. client A sends `UNSUBSCRIBE(sensor/+)`
3. disconnect happens before `UNSUBACK`
4. task broadcasts `Disconnected`
5. reconnect restore replays the pending `UNSUBSCRIBE`
6. router still keeps client A subscribed because no `UNSUBACK` has arrived yet
7. incoming publish on `sensor/temp` before `UNSUBACK` still reaches both clients
8. broker sends replayed `UNSUBACK`
9. driver emits `UnsubscribeAckFor { client_id: A, filters, completion }`
10. task removes that filter from the router only for client A
11. task routes `Completion::UnsubAck` only to client A
12. later incoming publish on `sensor/temp` reaches only client B

This timing is intentional.

Router mutation policy is:
- add routes on `SUBACK`
- remove routes on `UNSUBACK`
- do not mutate routes on disconnect alone
- do not mutate routes on replay alone

That policy avoids removing routes too early or installing routes optimistically before the broker has acknowledged them.
