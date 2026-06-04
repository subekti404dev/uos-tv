//! Volume control via ALSA direct API.
//!
//! On Linux: uses `alsa::mixer::Mixer` to control the default sound card.
//! Falls back to no-op if no ALSA card is available.

pub const MIN_VOLUME: u32 = 0;
pub const MAX_VOLUME: u32 = 100;
const DEFAULT_VOLUME: u32 = 70;
const VOLUME_STEP: u32 = 5;

pub struct VolumeController {
    volume: u32,
    muted: bool,
    previous_volume: u32,

    // ALSA mixer handle (lazy init)
    #[cfg(feature = "alsa-backend")]
    mixer: Option<alsa::mixer::Mixer>,

    // Cached selem_id for the "Master" control
    #[cfg(feature = "alsa-backend")]
    master_selem: Option<alsa::mixer::SelemId>,
}

impl VolumeController {
    pub fn new() -> Self {
        #[cfg(feature = "alsa-backend")]
        let (mixer, selem) = Self::open_alsa();

        Self {
            volume: DEFAULT_VOLUME,
            muted: false,
            previous_volume: DEFAULT_VOLUME,
            #[cfg(feature = "alsa-backend")]
            mixer,
            #[cfg(feature = "alsa-backend")]
            master_selem: selem,
        }
    }

    #[cfg(feature = "alsa-backend")]
    fn open_alsa() -> (Option<alsa::mixer::Mixer>, Option<alsa::mixer::SelemId>) {
        match alsa::mixer::Mixer::new("default", false) {
            Ok(mixer) => {
                let selem_id = alsa::mixer::SelemId::new("Master", 0);
                tracing::info!("ALSA: mixer 'default' opened, selem 'Master'");
                (Some(mixer), Some(selem_id))
            }
            Err(e) => {
                tracing::warn!("ALSA: could not open default mixer: {e} — volume control is no-op");
                (None, None)
            }
        }
    }

    #[cfg(not(feature = "alsa-backend"))]
    fn open_alsa() -> (Option<()>, Option<()>) {
        tracing::debug!("ALSA: not available — volume control is no-op");
        (None, None)
    }

    pub fn volume(&self) -> u32 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    pub fn set_volume(&mut self, vol: u32) {
        self.volume = vol.min(MAX_VOLUME).max(MIN_VOLUME);
        self.apply();
    }

    pub fn volume_up(&mut self) {
        self.volume = (self.volume + VOLUME_STEP).min(MAX_VOLUME);
        if self.muted {
            self.muted = false;
        }
        self.apply();
    }

    pub fn volume_down(&mut self) {
        self.volume = self.volume.saturating_sub(VOLUME_STEP).max(MIN_VOLUME);
        self.apply();
    }

    pub fn toggle_mute(&mut self) {
        if self.muted {
            self.muted = false;
            self.volume = self.previous_volume;
        } else {
            self.muted = true;
            self.previous_volume = self.volume;
            self.volume = 0;
        }
        self.apply();
    }

    /// Apply volume/mute to hardware via ALSA.
    #[cfg(feature = "alsa-backend")]
    fn apply(&self) {
        let long_vol: i64 = self.volume as i64;

        if let (Some(mixer), Some(selem_id)) = (&self.mixer, &self.master_selem) {
            if let Some(selem) = mixer.find_selem(selem_id) {
                let _ = selem.set_playback_switch_all(if self.muted { 0 } else { 1 });

                // Map 0..100 → ALSA raw range
                let (min, max) = selem.get_playback_volume_range();
                let raw = min + (long_vol * (max - min) / 100);
                let _ = selem.set_playback_volume_all(raw);
            }
        }

        tracing::debug!(
            "Volume: {}% {} (ALSA)",
            self.volume,
            if self.muted { "(muted)" } else { "" }
        );
    }

    #[cfg(not(feature = "alsa-backend"))]
    fn apply(&self) {
        tracing::debug!(
            "Volume: {}% {} (no-ALSA)",
            self.volume,
            if self.muted { "(muted)" } else { "" }
        );
    }

    /// Get sound card info (ALSA-level).
    #[cfg(feature = "alsa-backend")]
    pub fn cards(&self) -> Vec<CardInfo> {
        let mut cards = Vec::new();
        for item in alsa::card::Iter::new() {
            if let Ok(card) = item {
                let index = card.get_index();
                if let Ok(name) = card.get_name() {
                    cards.push(CardInfo { index, name });
                }
            }
        }
        cards
    }

    #[cfg(not(feature = "alsa-backend"))]
    pub fn cards(&self) -> Vec<CardInfo> {
        vec![]
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CardInfo {
    pub index: i32,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_control() {
        let mut vc = VolumeController::new();
        assert_eq!(vc.volume(), DEFAULT_VOLUME);

        vc.volume_up();
        assert_eq!(vc.volume(), DEFAULT_VOLUME + VOLUME_STEP);

        vc.volume_down();
        assert_eq!(vc.volume(), DEFAULT_VOLUME);

        vc.set_volume(999);
        assert_eq!(vc.volume(), MAX_VOLUME);

        vc.toggle_mute();
        assert!(vc.is_muted());
        assert_eq!(vc.volume(), 0);

        vc.toggle_mute();
        assert!(!vc.is_muted());
        assert_eq!(vc.volume(), MAX_VOLUME);
    }

    #[test]
    fn test_cards() {
        let vc = VolumeController::new();
        let cards = vc.cards();
        // On Linux without sound: 0 cards
        // On macOS: 0 cards
        // Either is fine
        assert!(cards.is_empty() || !cards.is_empty());
    }
}
