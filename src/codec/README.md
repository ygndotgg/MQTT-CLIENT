# MQTT v3.1.1 Codec Overview

This module implements a strict MQTT 3.1.1 wire codec:

- **Encode**: typed `Packet` values -> MQTT byte stream
- **Decode**: MQTT byte stream -> typed `Packet` values

It is designed to be deterministic, spec-driven, and defensive against malformed frames.

## Architecture

The codec is built around two dispatcher entry points:

- `encode_packet(packet, out)`
- `decode_packet(input)`

### Encode dispatcher

`encode_packet` pattern-matches on `Packet` enum variants and forwards to packet-specific encoders, for example:

- `CONNECT` -> `encode_connect`
- `PUBLISH` -> `encode_publish`
- ack-family packets -> shared helpers where appropriate

### Decode dispatcher

`decode_packet`:

1. Parses fixed header (`packet_type`, flags, remaining length)
2. Verifies full frame availability (`input.len() >= frame_len`)
3. Slices exactly the frame body
4. Dispatches by `packet_type` to packet-specific decoders

This keeps frame routing centralized while packet rules stay local.

## MQTT Frame Model

Each MQTT packet is decoded/encoded as:

1. **Fixed header byte1**
2. **Remaining length** (MQTT varint, 1..4 bytes)
3. **Body** (variable header + payload)

The fixed-header byte1 layout:

- high nibble (`byte1 >> 4`) = packet type
- low nibble (`byte1 & 0x0F`) = packet flags

## Bit-Level Behavior

Bit shifts and masks are used because MQTT packs multiple fields into one byte.

Examples:

- packet type: `byte1 >> 4`
- flags: `byte1 & 0x0F`
- PUBLISH flags:
  - `dup` = bit 3
  - `qos` = bits 1-2
  - `retain` = bit 0

Encoding packs fields back into byte positions using `<<` and `|`.

## Packet Families and Rules

## CONNECT / CONNACK

`CONNECT` validates:

- protocol name is `"MQTT"`
- protocol level is `4`
- reserved connect flag bit 0 is `0`
- will flag matrix consistency
- username/password consistency
- payload order and complete consumption (no extra bytes)

`CONNACK` validates:

- body length exactly `2`
- session-present uses only bit0
- return code in valid range (`0x00..=0x05`)
- strict semantic check for session-present with non-success code

## PUBLISH + QoS flow

`PUBLISH` rules:

- QoS bits must map to 0/1/2
- QoS0 has no packet identifier on wire
- QoS1/QoS2 require non-zero packet identifier

Ack-family:

- `PUBACK`, `PUBREC`, `PUBREL`, `PUBCOMP`
- remaining length exactly `2`
- non-zero packet identifier
- fixed-header flags:
  - `PUBREL` must be `0x2`
  - others must be `0x0`

## SUBSCRIBE / UNSUBSCRIBE lifecycle

`SUBSCRIBE`:

- fixed-header flags must be `0x2`
- non-zero packet identifier
- at least one topic filter
- each requested qos in 0/1/2

`UNSUBSCRIBE`:

- fixed-header flags must be `0x2`
- non-zero packet identifier
- at least one topic filter

`SUBACK`:

- fixed-header flags `0x0`
- non-zero packet identifier
- return codes restricted to `{0x00, 0x01, 0x02, 0x80}`

`UNSUBACK`:

- fixed-header flags `0x0`
- remaining length exactly `2`
- non-zero packet identifier

## PINGREQ / PINGRESP / DISCONNECT

Control packets with empty bodies:

- `PINGREQ` = `0xC0 0x00`
- `PINGRESP` = `0xD0 0x00`
- `DISCONNECT` = `0xE0 0x00`

Decoders enforce:

- fixed-header flags must be `0`
- remaining length must be `0`

## Error Model

The codec returns typed errors (never panics for malformed network data):

- `InsufficientBytes`
- `MalformedRemainingLength`
- `InvalidPacketType`
- `InvalidFixedHeaderFlags`
- `InvalidQos`
- `PacketIdZero`
- `InvalidProtocol`
- `InvalidProtocolLevel`
- `MalformedPacket`

This supports safe streaming decode behavior and clear rejection reasons.

## Testing Strategy

Implementation correctness is validated with:

- **Golden tests**: exact byte-level fixtures
- **Roundtrip tests**:
  - struct -> bytes -> struct
  - bytes -> struct -> bytes
- **Negative tests**: malformed flags/lengths/qos/codes/ids/truncation

This combination verifies both protocol compliance and decoder robustness.
