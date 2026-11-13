//! Planar stereo buffers.

/// Planar stereo `f32` at 48 kHz: both channels always the same length.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stereo {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Stereo {
    /// `frames` frames of silence.
    pub fn silence(frames: usize) -> Stereo {
        Stereo {
            l: vec![0.0; frames],
            r: vec![0.0; frames],
        }
    }

    /// The number of frames.
    pub fn len(&self) -> usize {
        debug_assert_eq!(self.l.len(), self.r.len());
        self.l.len()
    }

    pub fn is_empty(&self) -> bool {
        self.l.is_empty()
    }

    /// Empties both channels, keeping their capacity.
    pub fn clear(&mut self) {
        self.l.clear();
        self.r.clear();
    }

    /// Appends one frame.
    pub fn push(&mut self, l: f32, r: f32) {
        self.l.push(l);
        self.r.push(r);
    }
}
