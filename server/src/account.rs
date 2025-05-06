use anyhow::{bail, Result};
use common::web_api::{AccountCapabilityMode, DeviceCapabilityType};
use libsignal_core::{Aci, DeviceId, Pni, ProtocolAddress, ServiceIdKind};
use libsignal_protocol::IdentityKey;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Account {
    aci: Aci,
    pni: Pni,
    aci_identity_key: IdentityKey,
    pni_identity_key: IdentityKey,
    devices: Vec<Device>,
    phone_number: String,
}

impl Hash for Account {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.aci.hash(state);
    }
}

impl Account {
    pub fn new(
        pni: Pni,
        aci_identity_key: IdentityKey,
        pni_identity_key: IdentityKey,
        device: Device,
        phone_number: String,
    ) -> Self {
        Self {
            aci: Uuid::new_v4().into(),
            pni,
            aci_identity_key,
            pni_identity_key,
            devices: vec![device],
            phone_number,
        }
    }

    pub fn from_db(
        aci: Aci,
        pni: Pni,
        aci_identity_key: IdentityKey,
        pni_identity_key: IdentityKey,
        devices: Vec<Device>,
        phone_number: String,
    ) -> Self {
        Self {
            aci,
            pni,
            aci_identity_key,
            pni_identity_key,
            devices,
            phone_number,
        }
    }

    pub fn aci(&self) -> Aci {
        self.aci
    }

    pub fn pni(&self) -> Pni {
        self.pni
    }

    pub fn aci_identity_key(&self) -> IdentityKey {
        self.aci_identity_key
    }

    pub fn pni_identity_key(&self) -> IdentityKey {
        self.pni_identity_key
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn add_device(&mut self, device: Device) -> Result<()> {
        if self.devices.contains(&device) {
            bail!("Device is already registered on the account")
        }
        self.devices.push(device);
        Ok(())
    }

    pub fn phone_number(&self) -> &str {
        &self.phone_number
    }

    pub fn has_capability(&self, device_capability: &DeviceCapabilityType) -> bool {
        match device_capability.value().account_capability_mode {
            AccountCapabilityMode::PrimaryDevice => self
                .devices()
                .iter()
                .find(|device| device.device_id() == 1.into())
                .expect("User always has a primary device")
                .has_capability(device_capability),
            AccountCapabilityMode::AnyDevice => self
                .devices()
                .iter()
                .any(|device| device.has_capability(device_capability)),
            AccountCapabilityMode::AllDevices => self
                .devices()
                .iter()
                .all(|device| device.has_capability(device_capability)),
        }
    }

    pub fn get_next_device_id(&self) -> u32 {
        self.devices
            .iter()
            .map(|device| u32::from(device.device_id))
            .max()
            .expect("Will always have some device")
            + 1
    }

    pub fn get_protocol_address(
        &self,
        kind: ServiceIdKind,
        device_id: DeviceId,
    ) -> ProtocolAddress {
        ProtocolAddress::new(
            match kind {
                ServiceIdKind::Aci => self.aci.service_id_string(),
                ServiceIdKind::Pni => self.pni.service_id_string(),
            },
            device_id,
        )
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, bon::Builder)]
pub struct Device {
    device_id: DeviceId,
    name: String,
    last_seen: u128,
    created: u128,
    auth_token: String,
    salt: String,
    registration_id: u32,
    pni_registration_id: u32,
    capabilities: Vec<DeviceCapabilityType>,
}
impl Device {
    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }
    pub fn name(&self) -> &String {
        &self.name
    }
    pub fn last_seen(&self) -> u128 {
        self.last_seen
    }
    pub fn created(&self) -> u128 {
        self.created
    }

    pub fn auth_token(&self) -> &String {
        &self.auth_token
    }

    pub fn salt(&self) -> &str {
        &self.salt
    }

    pub fn registration_id(&self) -> u32 {
        self.registration_id
    }

    pub fn pni_registration_id(&self) -> u32 {
        self.pni_registration_id
    }

    pub fn capabilities(&self) -> Vec<DeviceCapabilityType> {
        self.capabilities.clone()
    }

    pub fn has_capability(&self, capability: &DeviceCapabilityType) -> bool {
        self.capabilities.contains(capability)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AuthenticatedDevice {
    account: Account,
    device: Device,
}

impl AuthenticatedDevice {
    pub fn new(account: Account, device: Device) -> Self {
        Self { account, device }
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn get_protocol_address(&self, kind: ServiceIdKind) -> ProtocolAddress {
        ProtocolAddress::new(
            match kind {
                ServiceIdKind::Aci => self.account().aci.service_id_string(),
                ServiceIdKind::Pni => self.account().pni.service_id_string(),
            },
            self.device().device_id(),
        )
    }
}
