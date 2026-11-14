//! The reader's sample trim (6C plan Task 12, Decision 22): the priming dropped at the start
//! and the presented length passed, so padding never reaches the caller.

/// Drops `skip` frames from the start, then passes at most `left` frames (the presented
/// length, when the container states it exactly) and drops the rest: padding is never held.
#[derive(Debug, Default)]
pub(super) struct Trim {
    pub(super) skip: u64,
    pub(super) left: Option<u64>,
}

impl Trim {
    pub(super) fn push(&mut self, pcm: &[f32], channels: usize) -> Vec<f32> {
        let frames = (pcm.len() / channels) as u64;
        let drop = self.skip.min(frames);
        self.skip -= drop;
        let mut take = frames - drop;
        if let Some(left) = &mut self.left {
            take = take.min(*left);
            *left -= take;
        }
        let from = drop as usize * channels;
        pcm[from..from + take as usize * channels].to_vec()
    }

    /// Everything presented has been passed.
    pub(super) fn done(&self) -> bool {
        self.left == Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::Trim;

    #[test]
    fn trim_skips_priming_and_stops_at_the_presented_length() {
        let mut t = Trim {
            skip: 3,
            left: Some(5),
        };
        let pcm: Vec<f32> = (0..10).map(|i| i as f32).collect();
        // Mono: frames 0..10 in chunks of 4, 4, 2.
        let mut out = t.push(&pcm[..4], 1);
        out.extend(t.push(&pcm[4..8], 1));
        out.extend(t.push(&pcm[8..], 1));
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0, 7.0]);
        assert!(t.done());
    }
}
