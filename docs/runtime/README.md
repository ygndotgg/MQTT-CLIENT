# Runtime Docs

This directory collects the runtime-side architecture notes for the MQTT client.

## Files

- `architecture.md`
  - high-level architecture and ownership model
- `event-loop.md`
  - end-to-end command, network, tick, and reconnect flow
- `driver.md`
  - `RuntimeDriver`, `DriverEvent`, and `DriverAction`
- `function-reference.md`
  - function-by-function responsibilities for client, host, task, driver, state, and inflight
- `qos1.md`
  - QoS1 runtime and inflight semantics
- `../runtime-qos2.md`
  - QoS2 sender and receiver state machines

## Suggested reading order

1. `architecture.md`
2. `event-loop.md`
3. `driver.md`
4. `function-reference.md`
5. `qos1.md`
6. `../runtime-qos2.md`

That order goes from broad ownership boundaries to detailed delivery-state behavior.
