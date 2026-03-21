use crate::{
    codec::{
        encode_remaining_length, parse_fixed_header, read_mqtt_bytes, read_mqtt_string, read_u8,
        read_u16, write_mqtt_bytes, write_mqtt_string, write_u16,
    },
    protocol::v4::error::Error,
    types::{
        ConnAck, Connect, LastWill, Packet, Publish, Qos, SubAck, Subscribe, UnsubAck, Unsubscribe,
    },
};

pub fn encode_packet(packet: &Packet, out: &mut Vec<u8>) -> Result<(), Error> {
    match packet {
        Packet::Connect(p) => encode_connect(&p, out),
        Packet::ConnAck(p) => encode_connack(&p, out),
        Packet::Publish(p) => encode_publish(&p, out),
        Packet::PubAck(p) => encode_pkid_only(4, p.pkid, 0, out),
        Packet::PubRec(p) => encode_pkid_only(5, p.pkid, 0, out),
        Packet::PubRel(p) => encode_pkid_only(6, p.pkid, 0x02, out),
        Packet::PubComp(p) => encode_pkid_only(7, p.pkid, 0, out),
        Packet::Subscribe(p) => encode_subscribe(p, out),
        Packet::SubAck(p) => encode_suback(p, out),
        Packet::UnSubscribe(p) => encode_unsubscribe(p, out),
        Packet::UnsubAck(p) => encode_unsuback(p, out),
        Packet::PingReq => encode_ping(12, out),
        Packet::PingResp => encode_ping(13, out),
        Packet::Disconnect => encode_disconnect(out),

        _ => unimplemented!(),
    }
}

fn encode_disconnect(out: &mut Vec<u8>) -> Result<(), Error> {
    out.push(0xE0); // type 14 + flags 0
    out.push(0x00);
    Ok(())
}

fn encode_connack(p: &ConnAck, out: &mut Vec<u8>) -> Result<(), Error> {
    if !matches!(p.code, 0x00..=0x05) {
        return Err(Error::InvalidConnAckCode(p.code));
    }
    if p.code != 0x00 && p.session_present {
        return Err(Error::MalformedPacket);
    }
    out.push(0x20);
    out.push(0x02);
    // --- Variable Header ---
    let ack_flags = if p.session_present { 0x01 } else { 0x00 };
    out.push(ack_flags);
    out.push(p.code);
    Ok(())
}

fn encode_connect(p: &Connect, out: &mut Vec<u8>) -> Result<(), Error> {
    let mut body = Vec::new();

    // ---- Variable header ----
    write_mqtt_string(&mut body, "MQTT");
    body.push(4);

    let flags = construct_flags(p)?;
    body.push(flags);

    write_u16(&mut body, p.keep_alive);

    // ---- Payload ----
    write_mqtt_string(&mut body, &p.client_id);

    if let Some(will) = &p.will {
        write_mqtt_string(&mut body, &will.topic);
        write_mqtt_bytes(&mut body, &will.message);
    }

    if let Some(username) = &p.username {
        write_mqtt_string(&mut body, username);

        if let Some(password) = &p.password {
            write_mqtt_bytes(&mut body, password);
        }
    }

    // ---- Fixed header ----
    out.push(0x10);
    out.extend_from_slice(&encode_remaining_length(body.len())?);
    out.extend_from_slice(&body);

    Ok(())
}

fn construct_flags(p: &Connect) -> Result<u8, Error> {
    let mut f = 0u8;

    // bit 1: clean session
    if p.clean_session {
        f |= 1 << 1;
    }

    // will handling
    if let Some(will) = &p.will {
        // bit 2: will flag
        f |= 1 << 2;

        // bits 3–4: QoS (2 bits)
        let qos = u8::from(will.qos);
        if qos > 2 {
            return Err(Error::InvalidQos(qos));
        }
        f |= (qos & 0b11) << 3;

        // bit 5: retain
        if will.retain {
            f |= 1 << 5;
        }
    }

    // username / password
    if p.username.is_some() {
        f |= 1 << 7;
    }

    if p.password.is_some() {
        f |= 1 << 6;
    }

    // invariant: password requires username
    if p.password.is_some() && p.username.is_none() {
        return Err(Error::InvalidProtocol);
    }

    // invariant: reserved bit must be 0
    debug_assert_eq!(f & 0x01, 0);

    Ok(f)
}

pub fn decode_packet(input: &[u8]) -> Result<Packet, Error> {
    let fh = parse_fixed_header(input)?;
    if input.len() < fh.frame_len() {
        return Err(Error::InsufficientBytes(fh.frame_len() - input.len()));
    }
    let body = &input[fh.header_len..fh.frame_len()];
    let flag = fh.flags();
    // assert!(input.len() >= fh.frame_len());
    // let body = input[fh.frame_len()..];
    match fh.packet_type() {
        1 => {
            if flag != 0 {
                return Err(Error::InvalidFixedHeaderFlags {
                    packet_type: 1,
                    flags: flag,
                });
            }
            Ok(Packet::Connect(decode_connect(body)?))
        }
        2 => {
            if flag != 0 {
                return Err(Error::InvalidFixedHeaderFlags {
                    packet_type: 2,
                    flags: flag,
                });
            }
            Ok(Packet::ConnAck(decode_connack(body)?))
        }
        3 => decode_publish(flag, body),
        4 => decode_puback_like(4, flag, body),
        5 => decode_puback_like(5, flag, body),
        6 => decode_puback_like(6, flag, body),
        7 => decode_puback_like(7, flag, body),
        8 => decode_subscribe(flag, body),
        9 => decode_suback(flag, body),
        10 => decode_unsubscribe(flag, body),
        11 => decode_unsuback(flag, body),
        12 => decode_ping(12, flag, body),
        13 => decode_ping(13, flag, body),
        14 => decode_disconnect(flag, body),
        t => Err(Error::InvalidPacketType(t)),
    }
}

pub fn decode_connect(body: &[u8]) -> Result<Connect, Error> {
    let mut curr = body;
    let protocol = read_mqtt_string(&mut curr)?;
    if protocol != "MQTT".to_owned() {
        return Err(Error::InvalidProtocol);
    }
    let level = read_u8(&mut curr)?;
    if level != 4 {
        return Err(Error::InvalidProtocolLevel(level));
    }
    let flags = read_u8(&mut curr)?;
    if (flags & 0x01) != 0 {
        return Err(Error::MalformedPacket);
    } // Reserved Bit must be Zero

    let clean_session = (flags & 0b0000_0010) != 0;
    let will_flag = (flags & 0b0000_0100) != 0;
    let will_qos = (flags >> 3) & 0b11;
    let will_retain = (flags & 0b0010_0000) != 0;
    let password_flag = (flags & 0b0100_0000) != 0;
    let username_flag = (flags & 0b1000_0000) != 0;

    // --- FEW EDGE CASES ---
    if !will_flag && (will_qos != 0 || will_retain) {
        return Err(Error::MalformedPacket);
    }
    if will_flag && will_qos > 2 {
        return Err(Error::InvalidQos(will_qos));
    }
    if password_flag && !username_flag {
        return Err(Error::MalformedPacket);
    }
    let keep_alive = read_u16(&mut curr)?;
    // --- PAYLOAD ---
    let client_id = read_mqtt_string(&mut curr)?;
    let will = if will_flag {
        let topic = read_mqtt_string(&mut curr)?;
        let message = read_mqtt_bytes(&mut curr)?;
        let qos = Qos::try_from(will_qos)?;
        Some(LastWill {
            topic,
            message,
            qos,
            retain: will_retain,
        })
    } else {
        None
    };
    let username = if username_flag {
        Some(read_mqtt_string(&mut curr)?)
    } else {
        None
    };

    let password = if password_flag {
        Some(read_mqtt_bytes(&mut curr)?)
    } else {
        None
    };

    if !curr.is_empty() {
        return Err(Error::MalformedPacket);
    }
    Ok(Connect {
        clean_session,
        keep_alive,
        client_id,
        will,
        username,
        password,
    })
}

fn decode_connack(body: &[u8]) -> Result<ConnAck, Error> {
    if body.len() != 2 {
        return Err(Error::MalformedPacket);
    }
    let ack_flags = body[0];
    if (ack_flags & 0xFE) != 0 {
        return Err(Error::MalformedPacket);
    }
    let session_present = (ack_flags & 0x01) != 0;
    let code = body[1];

    if !matches!(code, 0x00..=0x05) {
        return Err(Error::MalformedPacket);
    }
    if code != 0x00 && session_present {
        return Err(Error::MalformedPacket);
    }
    Ok(ConnAck {
        session_present,
        code,
    })
}

fn encode_pkid_only(packet_type: u8, pkid: u16, flags: u8, out: &mut Vec<u8>) -> Result<(), Error> {
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    out.push((packet_type << 4) | (flags & 0x0f));
    out.push(0x02);
    write_u16(out, pkid);
    Ok(())
}

fn encode_publish(p: &Publish, out: &mut Vec<u8>) -> Result<(), Error> {
    let qos = u8::from(p.qos);
    if qos > 2 {
        return Err(Error::InvalidQos(qos));
    }
    if qos == 0 && p.pkid != 0 {
        return Err(Error::MalformedPacket);
    }
    if qos > 0 && p.pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    let mut body = Vec::new();
    write_mqtt_string(&mut body, &p.topic);
    if qos > 0 {
        write_u16(&mut body, p.pkid);
    }
    body.extend_from_slice(&p.payload);
    let flags = ((p.dup as u8) << 3) | ((qos & 0b11) << 1) | (p.retain as u8);
    out.push((3 << 4) | flags);
    out.extend_from_slice(&encode_remaining_length(body.len())?);
    out.extend_from_slice(&body);
    Ok(())
}

fn decode_publish(flags: u8, body: &[u8]) -> Result<Packet, Error> {
    let dup = (flags & 0b1000) != 0;
    let qos_bits = (flags >> 1) & 0b11;
    let retain = (flags & 0b0001) != 0;
    let qos = Qos::try_from(qos_bits)?;
    let mut curr = body;
    let topic = read_mqtt_string(&mut curr)?;
    let pkid = if qos_bits > 0 {
        let id = read_u16(&mut curr)?;
        if id == 0 {
            return Err(Error::PacketIdZero);
        }
        id
    } else {
        0
    };
    let payload = curr.to_vec();
    Ok(Packet::Publish(Publish {
        dup,
        qos,
        retain,
        topic,
        pkid,
        payload,
    }))
}

fn decode_puback_like(packet_type: u8, flags: u8, body: &[u8]) -> Result<Packet, Error> {
    let required_flags = if packet_type == 6 { 0b0010 } else { 0 };
    if flags != required_flags {
        return Err(Error::InvalidFixedHeaderFlags { packet_type, flags });
    }
    if body.len() != 2 {
        return Err(Error::MalformedPacket);
    }
    let mut curr = body;
    let pkid = read_u16(&mut curr)?;
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    Ok(match packet_type {
        4 => Packet::PubAck(crate::types::PubAck { pkid }),
        5 => Packet::PubRec(crate::types::PubRec { pkid }),
        6 => Packet::PubRel(crate::types::PubRel { pkid }),
        7 => Packet::PubComp(crate::types::PubComp { pkid }),
        _ => return Err(Error::InvalidPacketType(packet_type)),
    })
}

fn encode_subscribe(p: &Subscribe, out: &mut Vec<u8>) -> Result<(), Error> {
    if p.pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    if p.filters.is_empty() {
        return Err(Error::MalformedPacket);
    }

    let mut body = Vec::new();
    write_u16(&mut body, p.pkid);
    for i in &p.filters {
        let k: u8 = i.1.into();
        if k > 2 {
            return Err(Error::InvalidQos(k));
        }

        write_mqtt_string(&mut body, &i.0);
        body.push(k);
    }
    out.push(0x82);
    out.extend_from_slice(&encode_remaining_length(body.len())?);
    out.extend_from_slice(&body);
    Ok(())
}

fn encode_suback(p: &SubAck, out: &mut Vec<u8>) -> Result<(), Error> {
    if p.pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    if p.return_codes.is_empty() {
        return Err(Error::MalformedPacket);
    }
    if !p.return_codes.iter().copied().all(valid_suback_code) {
        return Err(Error::MalformedPacket);
    }
    let mut body = Vec::new();
    write_u16(&mut body, p.pkid);
    body.extend_from_slice(&p.return_codes);
    out.push(0x90);
    out.extend_from_slice(&encode_remaining_length(body.len())?);
    out.extend_from_slice(&body);
    Ok(())
}

fn encode_unsubscribe(p: &Unsubscribe, out: &mut Vec<u8>) -> Result<(), Error> {
    if p.pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    if p.filters.is_empty() {
        return Err(Error::MalformedPacket);
    }
    let mut body = Vec::new();
    write_u16(&mut body, p.pkid);
    for topic in &p.filters {
        write_mqtt_string(&mut body, topic);
    }
    out.push(0xA2);
    out.extend_from_slice(&encode_remaining_length(body.len())?);
    out.extend_from_slice(&body);
    Ok(())
}

fn encode_unsuback(p: &UnsubAck, out: &mut Vec<u8>) -> Result<(), Error> {
    if p.pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    out.push(0xB0); // type 11 + flag 0 
    out.push(0x02); // remaining len exactly 2 
    write_u16(out, p.pkid);
    Ok(())
}
fn decode_subscribe(f: u8, body: &[u8]) -> Result<Packet, Error> {
    if f != 0x02 {
        return Err(Error::InvalidFixedHeaderFlags {
            packet_type: 8,
            flags: f,
        });
    }
    let mut curr = body;
    let pkid = read_u16(&mut curr)?;
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    let mut filters = Vec::new();
    while !curr.is_empty() {
        let topic = read_mqtt_string(&mut curr)?;
        let q = read_u8(&mut curr)?;
        let qos = Qos::try_from(q)?;
        filters.push((topic, qos));
    }
    if filters.is_empty() {
        return Err(Error::MalformedPacket);
    }
    Ok(Packet::Subscribe(Subscribe { pkid, filters }))
}
fn decode_unsubscribe(f: u8, body: &[u8]) -> Result<Packet, Error> {
    if f != 0x02 {
        return Err(Error::InvalidFixedHeaderFlags {
            packet_type: 10,
            flags: f,
        });
    }
    let mut curr = body;
    let pkid = read_u16(&mut curr)?;
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    let mut filters = Vec::new();
    while !curr.is_empty() {
        filters.push(read_mqtt_string(&mut curr)?);
    }
    if filters.is_empty() {
        return Err(Error::MalformedPacket);
    }
    Ok(Packet::UnSubscribe(Unsubscribe { pkid, filters }))
}
fn decode_suback(f: u8, body: &[u8]) -> Result<Packet, Error> {
    if f != 0 {
        return Err(Error::InvalidFixedHeaderFlags {
            packet_type: 9,
            flags: f,
        });
    }
    let mut curr = body;
    let pkid = read_u16(&mut curr)?;
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    let return_codes = curr.to_vec();
    if return_codes.is_empty() {
        return Err(Error::MalformedPacket);
    }
    if !return_codes.iter().all(|c| valid_suback_code(*c)) {
        return Err(Error::MalformedPacket);
    }
    Ok(Packet::SubAck(SubAck { pkid, return_codes }))
}
fn decode_unsuback(f: u8, body: &[u8]) -> Result<Packet, Error> {
    if f != 0 {
        return Err(Error::InvalidFixedHeaderFlags {
            packet_type: 11,
            flags: f,
        });
    }
    if body.len() != 2 {
        return Err(Error::MalformedPacket);
    }
    let mut curr = body;
    let pkid = read_u16(&mut curr)?;
    if pkid == 0 {
        return Err(Error::PacketIdZero);
    }
    Ok(Packet::UnsubAck(UnsubAck { pkid }))
}

fn valid_suback_code(c: u8) -> bool {
    matches!(c, 0x00 | 0x01 | 0x02 | 0x80)
}

fn encode_ping(packet_type: u8, out: &mut Vec<u8>) -> Result<(), Error> {
    out.push(packet_type << 4);
    out.push(0x00);
    Ok(())
}

fn decode_ping(packet_type: u8, flags: u8, body: &[u8]) -> Result<Packet, Error> {
    if flags != 0 {
        return Err(Error::InvalidFixedHeaderFlags { packet_type, flags });
    }
    if !body.is_empty() {
        return Err(Error::MalformedPacket);
    }
    Ok(match packet_type {
        12 => Packet::PingReq,
        13 => Packet::PingResp,
        _ => return Err(Error::InvalidPacketType(packet_type)),
    })
}

fn decode_disconnect(f: u8, body: &[u8]) -> Result<Packet, Error> {
    if f != 0 {
        return Err(Error::InvalidFixedHeaderFlags {
            packet_type: 14,
            flags: f,
        });
    }
    if !body.is_empty() {
        return Err(Error::MalformedPacket);
    }
    Ok(Packet::Disconnect)
}
