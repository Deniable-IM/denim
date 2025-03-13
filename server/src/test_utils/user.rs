use crate::account::{Account, AuthenticatedDevice, Device};
use libsignal_core::{Aci, DeviceId, Pni, ProtocolAddress};
use libsignal_protocol::{IdentityKey, KeyPair};
use rand::{
    rngs::{OsRng, StdRng},
    Rng, SeedableRng,
};
use uuid::Uuid;

pub fn new_authenticated_device() -> AuthenticatedDevice {
    let (acc, device) = new_account_and_device();
    AuthenticatedDevice::new(acc, device)
}
pub fn new_account_and_device_and_address() -> (Account, Device, ProtocolAddress) {
    let (acc, device) = new_account_and_device();
    let address = ProtocolAddress::new(acc.aci().service_id_string(), device.device_id());
    (acc, device, address)
}
pub fn new_account_and_address() -> (Account, ProtocolAddress) {
    let (acc, _, address) = new_account_and_device_and_address();
    (acc, address)
}
pub fn new_account_and_device() -> (Account, Device) {
    let acc = new_account();
    let device = acc.devices()[0].clone();
    (acc, device)
}
pub fn new_account() -> Account {
    let mut csprng = OsRng;
    let identity_key = KeyPair::generate(&mut csprng);

    Account::new(
        new_pni(),
        IdentityKey::new(identity_key.public_key),
        IdentityKey::new(identity_key.public_key),
        new_device(),
        new_uuid().into(),
    )
}
pub fn new_device() -> Device {
    Device::builder()
        .device_id(new_device_id())
        .name("device".into())
        .last_seen(0)
        .created(0)
        .auth_token("bob_token".into())
        .salt("bob_salt".into())
        .registration_id(new_rand_number())
        .pni_registration_id(new_rand_number())
        .capabilities(Vec::new())
        .build()
}

pub fn new_protocol_address() -> ProtocolAddress {
    let name = new_aci();
    ProtocolAddress::new(name.service_id_string(), new_device_id())
}

pub fn new_aci() -> Aci {
    Aci::from(new_uuid())
}

pub fn new_pni() -> Pni {
    Pni::from(new_uuid())
}

pub fn new_device_id() -> DeviceId {
    new_rand_number().into()
}

pub fn new_rand_number() -> u32 {
    StdRng::from_entropy().gen::<u32>()
}

pub fn new_uuid() -> Uuid {
    Uuid::new_v4()
}
