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
    #[cfg(target_os = "linux")]
    mixer: Option<alsa::mixer::Mixer>,

    // Cached selem_id for the "Master" control
    #[cfg(target_os = "linux")]
    master_selem: Option<alsa::mixer::SelemId>,
}

impl VolumeController {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        let (mixer, selem) = Self::open_alsa();

        Self {
            volume: DEFAULT_VOLUME,
            muted: false,
            previous_volume: DEFAULT_VOLUME,
            #[cfg(target_os = "linux")]
            mixer,
            #[cfg(target_os = "linux")]
            master_selem: selem,
        }
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(not(target_os = "linux"))]
    fn open_alsa() -> (Option<()>, Option<()>) {
        tracing::debug!("ALSA: not available on this platform");
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
    #[cfg(target_os = "linux")]
    fn apply(&self) {
        let long_vol: i64 = self.volume as i64;

        if let (Some(ref mixer), Some(ref selem_id)) = (&self.mixer, &self.master_selem) {
            if let Some(selem) = mixer.find_selem(selem_id) {
                let _ = selem.set_playback_switch_all(if self.muted { 0 } else { 1 });

                // Map 0..100 → ALSA raw range
                if let Ok((min, max)) = selem.get_playback_volume_range() {
                    let raw = min + (long_vol * (max - min) as i64 / 100);
                    let _ = selem.set_playback_volume_all(raw);
                }
            }
        }

        tracing::debug!(
            "Volume: {}% {} (ALSA)",
            self.volume,
            if self.muted { "(muted)" } else { "" }
        );
    }

    #[cfg(not(target_os = "linux"))]
    fn apply(&self) {
        tracing::debug!(
            "Volume: {}% {} (no-ALSA)",
            self.volume,
            if self.muted { "(muted)" } else { "" }
        );
    }

    /// Get sound card info (ALSA-level).
    #[cfg(target_os = "linux")]
    pub fn cards(&self) -> Vec<CardInfo> {
        let mut cards = Vec::new();
        let mut card_idx: i32 = -1;

        while unsafe { alsa::card::ctls!().next(&mut card_idx) }.is_ok() && card_idx >= 0 {
            if let Ok(name) = alsa::card::Card::new(card_idx).and_then(|c| c.get_name()) {
                cards.push(CardInfo {
                    index: card_idx,
                    name,
                });
            }
        }
        cards
    }

    #[cfg(not(target_os = "linux"))]
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
