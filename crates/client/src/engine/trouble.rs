use crate::{
    audio::AudioError,
    engine::{EngineError, Side},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioTrouble {
    Missing(Side),
    Unplugged(Side, String),
    Broken(Side),
    Failed,
}

impl AudioTrouble {
    pub fn side(&self) -> Option<Side> {
        match self {
            Self::Missing(side) | Self::Unplugged(side, _) | Self::Broken(side) => Some(*side),
            Self::Failed => None,
        }
    }
}

impl From<&EngineError> for AudioTrouble {
    fn from(error: &EngineError) -> Self {
        let (side, error) = match error {
            EngineError::Microphone(error) => (Side::Microphone, error),
            EngineError::Speakers(error) => (Side::Speakers, error),
            EngineError::Resample(_) | EngineError::Codec(_) | EngineError::Thread(_) => {
                return Self::Failed;
            }
        };
        match error {
            AudioError::NoDevice(_) => Self::Missing(side),
            AudioError::NotFound { name, .. } => Self::Unplugged(side, name.clone()),
            AudioError::Format(_) | AudioError::Backend(_) => Self::Broken(side),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_that_failed_is_named() {
        let unplugged = EngineError::Microphone(AudioError::NotFound {
            kind: "input",
            name: "Yeti".into(),
        });
        assert_eq!(
            AudioTrouble::from(&unplugged),
            AudioTrouble::Unplugged(Side::Microphone, "Yeti".into())
        );

        let windows = EngineError::Speakers(AudioError::Backend(
            "Failed to get audio client: Cannot change thread mode after it is set.".into(),
        ));
        assert_eq!(
            AudioTrouble::from(&windows),
            AudioTrouble::Broken(Side::Speakers)
        );

        let none = EngineError::Speakers(AudioError::NoDevice("output"));
        assert_eq!(
            AudioTrouble::from(&none),
            AudioTrouble::Missing(Side::Speakers)
        );
        assert_eq!(AudioTrouble::from(&none).side(), Some(Side::Speakers));
    }

    #[test]
    fn trouble_inside_the_program_has_no_device() {
        let thread = EngineError::Thread(std::io::Error::other("no threads left"));
        assert_eq!(AudioTrouble::from(&thread), AudioTrouble::Failed);
        assert_eq!(AudioTrouble::Failed.side(), None);
    }
}
