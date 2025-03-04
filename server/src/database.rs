use crate::account::{Account, Device};
use anyhow::Result;
use axum::async_trait;
use common::signalservice::Envelope;
use common::web_api::{DeviceCapabilityType, DevicePreKeyBundle, UploadPreKey, UploadSignedPreKey};
use libsignal_core::{Aci, Pni, ProtocolAddress, ServiceId};

/// Represents a database connection that can store objects related to the signal protocol.
#[async_trait]
pub trait SignalDatabase: Clone + Send + Sync + 'static {
    /// Save a new account to the database.
    async fn add_account(&self, account: &Account) -> Result<()>;

    /// Get an account that was saved in the database.
    ///
    /// Beware: the account can be saved using either ACI, PNI or both,
    /// So if you search for an account using an ACI, you will not find
    /// it if the account only has a PNI in the database.
    async fn get_account(&self, service_id: &ServiceId) -> Result<Account>;

    async fn get_account_from_phonenumber_without_devices(
        &self,
        phone_number: &str,
    ) -> Result<Account>;

    /// Add device
    async fn add_device(&self, service_id: &ServiceId, device: &Device) -> Result<()>;
    /// Get all devices owned by account
    async fn get_all_devices(&self, service_id: &ServiceId) -> Result<Vec<Device>>;
    /// Get single device owned by account
    async fn get_device(&self, address: &ProtocolAddress) -> Result<Device>;
    /// Delete device owned by account
    async fn delete_device(&self, address: &ProtocolAddress) -> Result<()>;

    /// Add an ACI to an account overriding the existing ACI if any.
    async fn update_account_aci(&self, service_id: &ServiceId, new_aci: Aci) -> Result<()>;

    /// Add an PNI to an account overriding the existing PNI if any.
    async fn update_account_pni(&self, service_id: &ServiceId, new_pni: Pni) -> Result<()>;

    /// Delete the account associated with the given [ServiceId].
    async fn delete_account(&self, service_id: &ServiceId) -> Result<()>;

    async fn get_device_capabilities(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Vec<DeviceCapabilityType>>;

    async fn get_all_device_capabilities(
        &self,
        service_id: &ServiceId,
    ) -> Result<Vec<(i32, DeviceCapabilityType)>>;

    async fn add_used_device_link_token(&self, device_link_token: String) -> Result<()>;

    /// Send a message to a given [ProtocolAddress].
    async fn push_message_queue(
        &self,
        address: &ProtocolAddress,
        messages: Vec<Envelope>,
    ) -> Result<()>;

    /// Retreive a message that was sent to the given [ProtocolAddress].
    async fn pop_msg_queue(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>>;

    /// Stores a single signed pre key
    async fn store_signed_pre_key(
        &self,
        spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()>;

    /// Stores a single pq signed pre key
    async fn store_pq_signed_pre_key(
        &self,
        pq_spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()>;

    /// Store the keys that are needed to start a conversation with the device that
    /// corrosponds to the given [ProtocolAddress].
    async fn store_key_bundle(
        &self,
        data: &DevicePreKeyBundle,
        address: &ProtocolAddress,
    ) -> Result<()>;

    /// Get the keys that are needed to start a conversation with the device that
    /// corrosponds to the given [ProtocolAddress].
    async fn get_key_bundle(&self, address: &ProtocolAddress) -> Result<DevicePreKeyBundle>;

    /// Get how many keys are left until a last resort key is used instead of
    /// a one time prekey. More keys should be uploaded when this value is below
    /// some threshold.
    async fn get_one_time_ec_pre_key_count(&self, service_id: &ServiceId) -> Result<u32>;

    async fn get_one_time_pq_pre_key_count(&self, service_id: &ServiceId) -> Result<u32>;

    /// Store new one time prekeys to avoid running out.
    async fn store_one_time_ec_pre_keys(
        &self,
        otpks: Vec<UploadPreKey>,
        owner: &ProtocolAddress,
    ) -> Result<()>;

    async fn store_one_time_pq_pre_keys(
        &self,
        otpks: Vec<UploadSignedPreKey>,
        owner: &ProtocolAddress,
    ) -> Result<()>;

    /// Get a one time prekey so that you can start a conversation with the
    /// device that is associated with the given [ProtocolAddress].
    async fn get_one_time_ec_pre_key(
        &self,
        owner: &ProtocolAddress,
    ) -> Result<Option<UploadPreKey>>;

    async fn get_one_time_pq_pre_key(&self, owner: &ProtocolAddress) -> Result<UploadSignedPreKey>;

    /// Get number of messages for associated [ProtocolAddress]
    async fn count_messages(&self, address: &ProtocolAddress) -> Result<u32>;

    /// Get all messages for associated [ProtocolAddress]
    async fn get_messages(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>>;

    /// Delete and get all messages for associated [ProtocolAddress]
    async fn delete_messages(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>>;
}
