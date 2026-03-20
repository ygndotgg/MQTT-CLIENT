use mqtt_client::{protocol::v4::error::Error, types::Qos};

#[test]
fn t_qos_try_from_valid_values() {
    assert_eq!(Qos::try_from(0).unwrap(), Qos::AtMostOnce);
    assert_eq!(Qos::try_from(1).unwrap(), Qos::AtLeastOnce);
    assert_eq!(Qos::try_from(2).unwrap(), Qos::ExactlyOnce);
}

#[test]
fn t_qos_try_from_invalid_values() {
    assert_eq!(Qos::try_from(3).unwrap_err(), Error::InvalidQos(3));
    assert_eq!(Qos::try_from(7).unwrap_err(), Error::InvalidQos(7));
}

#[test]
fn t_qos_from() {
    let a: u8 = Qos::AtMostOnce.into();
    let b: u8 = Qos::AtLeastOnce.into();
    let c: u8 = Qos::ExactlyOnce.into();
    assert_eq!((a, b, c), (0, 1, 2));
}
