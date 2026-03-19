use crate::protocol::v4::error::Error;
const MAX_REMAINING_LEN: usize = 268_435_455;

pub fn read_u8(input: &mut &[u8]) -> Result<u8, Error> {
    if input.is_empty() {
        return Err(Error::InsufficientBytes(1));
    }
    let v = input[0];
    *input = &input[1..];
    Ok(v)
}

pub fn read_u16(input: &mut &[u8]) -> Result<u16, Error> {
    if input.len() < 2 {
        return Err(Error::InsufficientBytes(2 - input.len()));
    }

    let v = u16::from_be_bytes([input[0], input[1]]);
    *input = &input[2..];
    Ok(v)
}

pub fn read_mqtt_bytes(input: &mut &[u8]) -> Result<Vec<u8>, Error> {
    let len = read_u16(input)? as usize;
    if input.len() < len {
        return Err(Error::InsufficientBytes(len - input.len()));
    }
    let bytes = input[..len].to_vec();
    *input = &input[len..];
    Ok(bytes)
}

pub fn write_mqtt_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    write_u16(out, bytes.len() as u16);
    out.extend_from_slice(bytes);
}

pub fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub fn read_mqtt_string(input: &mut &[u8]) -> Result<String, Error> {
    let bytes = read_mqtt_bytes(input)?;
    let s = std::str::from_utf8(&bytes).map_err(|_| Error::MalformedPacket)?;
    Ok(s.to_owned())
}

pub fn write_mqtt_string(out: &mut Vec<u8>, s: &str) {
    write_mqtt_bytes(out, s.as_bytes());
}

pub fn encode_remaining_length(mut len: usize) -> Result<Vec<u8>, Error> {
    if len > MAX_REMAINING_LEN {
        return Err(Error::MalformedRemainingLength);
    }
    let mut out = Vec::with_capacity(4);
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        };
    }
    Ok(out)
}

pub fn decode_remaining_length(input: &[u8]) -> Result<(usize, usize), Error> {
    let mut multiplier = 1usize;
    let mut value = 0usize;
    for i in 0..4 {
        if i >= input.len() {
            return Err(Error::InsufficientBytes(1));
        }
        let byte = input[i];
        value += ((byte & 0x7F) as usize) * multiplier;
        if (byte & 0x80) == 0 {
            if value > MAX_REMAINING_LEN {
                return Err(Error::MalformedRemainingLength);
            }
            return Ok((value, i + 1));
        }
        multiplier *= 128;
    }
    Err(Error::MalformedRemainingLength)
}
