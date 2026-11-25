use pakery_core::crypto::{CpaceGroup, Hash};
use pakery_crypto::{P256Group, Sha512Hash, Spake2P256};
use pakery_spake2::{PartyA, PartyAState, PartyB, PartyBState, Spake2Error, Spake2Output};

use crate::NetworkError;

const IDENTITY_A: &str = "rfsync-identity-a";
const IDENTITY_B: &str = "rfsync-identity-b";
const AAD: &str = "rfsync-pairing-v1";

pub enum PartyState {
    PartyA(PartyAState<Spake2P256>),
    PartyB(PartyBState<Spake2P256>),
}

impl PartyState {
    pub fn finish(self, message: &[u8]) -> Result<Spake2Output, NetworkError> {
        let output = match self {
            Self::PartyA(state) => state.finish(message)?,
            Self::PartyB(state) => state.finish(message)?,
        };

        Ok(output)
    }
}

pub fn start_a(code: &str) -> Result<(Vec<u8>, PartyState), NetworkError> {
    let w = generate_w(code)?;
    let mut rng = rand::rng();

    let (message, state) = PartyA::<Spake2P256>::start(
        &w,
        IDENTITY_A.as_bytes(),
        IDENTITY_B.as_bytes(),
        AAD.as_bytes(),
        &mut rng,
    )?;

    Ok((message, PartyState::PartyA(state)))
}

pub fn start_b(code: &str) -> Result<(Vec<u8>, PartyState), NetworkError> {
    let w = generate_w(code)?;
    let mut rng = rand::rng();

    let (message, state) = PartyB::<Spake2P256>::start(
        &w,
        IDENTITY_A.as_bytes(),
        IDENTITY_B.as_bytes(),
        AAD.as_bytes(),
        &mut rng,
    )?;

    Ok((message, PartyState::PartyB(state)))
}

fn generate_w(code: &str) -> Result<<P256Group as CpaceGroup>::Scalar, NetworkError> {
    let hash = Sha512Hash::digest(code.as_bytes());
    let w = P256Group::scalar_from_wide_bytes(&hash).map_err(Spake2Error::from)?;
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::{start_a, start_b};
    use pakery_spake2::Spake2Error;
    use std::error::Error;

    #[test]
    fn 코드가_일치하면_양쪽_피어를_확정한다() -> Result<(), Box<dyn Error>> {
        let (message_a, state_a) = start_a("00123456")?;
        let (message_b, state_b) = start_b("00123456")?;

        let output_a = state_a.finish(&message_b)?;
        let output_b = state_b.finish(&message_a)?;

        output_a.verify_peer_confirmation(&output_b.confirmation_mac)?;
        output_b.verify_peer_confirmation(&output_a.confirmation_mac)?;
        assert!(output_a.session_key.as_bytes() == output_b.session_key.as_bytes());
        Ok(())
    }

    #[test]
    fn 코드가_다르면_양쪽_확인을_모두_거부한다() -> Result<(), Box<dyn Error>> {
        let (message_a, state_a) = start_a("00123456")?;
        let (message_b, state_b) = start_b("00123457")?;

        let output_a = state_a.finish(&message_b)?;
        let output_b = state_b.finish(&message_a)?;

        assert!(matches!(
            output_a.verify_peer_confirmation(&output_b.confirmation_mac),
            Err(Spake2Error::ConfirmationFailed)
        ));
        assert!(matches!(
            output_b.verify_peer_confirmation(&output_a.confirmation_mac),
            Err(Spake2Error::ConfirmationFailed)
        ));
        Ok(())
    }
}
