use crate::{
    errors::{DecodeContentError, DecodeDataMessageError, DecodeEnvelopeError},
    signalservice::{envelope::Type, Content, DataMessage, Envelope},
    SignalError,
};
use libsignal_protocol::Pni;
use libsignal_protocol::ServiceId;
use libsignal_protocol::{
    message_decrypt, CiphertextMessage, DeviceId, IdentityKeyStore, KyberPreKeyStore, PreKeyStore,
    ProtocolAddress, SessionStore, SignedPreKeyStore,
};
use prost::Message;
use rand::{CryptoRng, Rng};

pub struct ProcessedEnvelope {
    pub r#type: Option<Type>,
    pub source_service_id: Option<ServiceId>,
    pub source_device: Option<DeviceId>,
    pub destination_service_id: Option<ServiceId>,
    pub timestamp: Option<u64>,
    pub content: Option<Content>,
    pub server_guid: Option<String>,
    pub server_timestamp: Option<u64>,
    pub ephemeral: Option<bool>,
    pub urgent: Option<bool>,
    pub updated_pni: Option<Pni>,
    pub story: Option<bool>,
    pub reporting_token: Option<Vec<u8>>,
}

impl ProcessedEnvelope {
    pub fn try_get_message_as_string(&self) -> Result<String, SignalError> {
        Ok(self
            .content()?
            .data_message()?
            .body
            .ok_or(DecodeDataMessageError(
                "Could not decode message body - no message body.".to_owned(),
            ))?)
    }
    pub fn try_get_name_as_string(&self) -> Result<String, SignalError> {
        Ok(self.content()?.data_message()?.contact[0]
            .name
            .clone()
            .ok_or(DecodeDataMessageError(
                "Could not decode message contact - no message contact.".to_owned(),
            ))?
            .display_name()
            .to_owned())
    }
    pub fn source_service_id(&self) -> Result<ServiceId, DecodeEnvelopeError> {
        self.source_service_id
            .ok_or(DecodeEnvelopeError("No ServiceId in Content".to_owned()))
    }
    pub fn content(&self) -> Result<Content, DecodeEnvelopeError> {
        self.content
            .clone()
            .ok_or(DecodeEnvelopeError("No content in Envelope.".to_owned()))
    }
}

pub fn unpad_message(message: &[u8]) -> Result<Vec<u8>, SignalError> {
    for i in 0..message.len() {
        if message[i] == 0x80 {
            return Ok(message[0..i].to_vec());
        }
    }
    Err(SignalError::UnpadError)
}

impl Envelope {
    pub async fn decrypt<R: Rng + CryptoRng>(
        self,
        session_store: &mut dyn SessionStore,
        identity_store: &mut dyn IdentityKeyStore,
        pre_key_store: &mut dyn PreKeyStore,
        signed_pre_key_store: &mut dyn SignedPreKeyStore,
        kyber_pre_key_store: &mut dyn KyberPreKeyStore,
        csprng: &mut R,
    ) -> Result<ProcessedEnvelope, SignalError> {
        let r#type = if let Some(r#type) = self.r#type {
            Type::try_from(r#type)
                .map_err(|_| DecodeEnvelopeError("Unknown message type in envelope.".to_owned()))?
        } else {
            Err(DecodeEnvelopeError(
                "No message type in envelope.".to_owned(),
            ))?
        };

        let source_service_id = self
            .source_service_id
            .as_ref()
            .and_then(|string| ServiceId::parse_from_service_id_string(&string));

        let source_device = self.source_device.map(|int| int.into());

        let destination_service_id = self
            .destination_service_id
            .as_ref()
            .and_then(|string| ServiceId::parse_from_service_id_string(&string));

        let updated_pni = self
            .updated_pni
            .as_ref()
            .and_then(|string| Pni::parse_from_service_id_string(&string));

        let remote_address = match (source_service_id, source_device) {
            (Some(service_id), Some(device_id)) => {
                ProtocolAddress::new(service_id.service_id_string(), device_id)
            }
            (None, _) => Err(DecodeEnvelopeError("Missing Service ID.".to_owned()))?,
            (_, None) => Err(DecodeEnvelopeError("Missing Device ID.".to_owned()))?,
        };

        let content_bytes = self
            .content
            .ok_or(DecodeEnvelopeError("No content in message.".to_owned()))?;

        let ciphertext = CiphertextMessage::decode(content_bytes, r#type)?;

        let padded_msg = message_decrypt(
            &ciphertext,
            &remote_address,
            session_store,
            identity_store,
            pre_key_store,
            signed_pre_key_store,
            kyber_pre_key_store,
            csprng,
        )
        .await?;

        let content = Content::decode(unpad_message(padded_msg.as_slice().as_ref())?.as_ref())?;

        Ok(ProcessedEnvelope {
            r#type: Some(r#type),
            source_service_id,
            source_device,
            destination_service_id,
            timestamp: self.timestamp,
            content: Some(content),
            server_guid: self.server_guid,
            server_timestamp: self.server_timestamp,
            ephemeral: self.ephemeral,
            urgent: self.urgent,
            updated_pni,
            story: self.story,
            reporting_token: self.reporting_token,
        })
    }
}

pub trait DecodeableFromEnvelopeType: Sized {
    fn decode(bytes: Vec<u8>, r#type: Type) -> Result<Self, SignalError>;
}

impl DecodeableFromEnvelopeType for CiphertextMessage {
    fn decode(bytes: Vec<u8>, r#type: Type) -> Result<Self, SignalError> {
        match r#type {
            Type::Ciphertext => Ok(Self::SignalMessage((&*bytes).try_into()?)),
            Type::KeyExchange => Ok(Self::SenderKeyMessage((&*bytes).try_into()?)),
            Type::PrekeyBundle => Ok(Self::PreKeySignalMessage((&*bytes).try_into()?)),
            Type::PlaintextContent => Ok(Self::PlaintextContent((&*bytes).try_into()?)),
            Type::Unknown => Err(DecodeEnvelopeError(
                "Invalid Message type: Unknown".to_owned(),
            ))?,
            Type::Receipt => Err(DecodeEnvelopeError(
                "Invalid Message type: Receipt".to_owned(),
            ))?,
            Type::UnidentifiedSender => Err(DecodeEnvelopeError(
                "Invalid Message type: UnidentifiedSender".to_owned(),
            ))?,
        }
    }
}

impl Content {
    pub fn data_message(self) -> Result<DataMessage, DecodeContentError> {
        self.data_message
            .ok_or(DecodeContentError("No data message in Content".to_owned()))
    }
}
