//! A/B Judge: runs the same task on two (provider, model) tuples and picks
//! the better output based on a configurable signal (diff, tests, LLM judge).

use anyhow::Result;

pub struct AbJudge;

impl AbJudge {
    /// Pick the winner between two task outputs. Returns the index (0 or 1).
    /// `outputs` is a parallel array of (diff, test_passed) for each run.
    pub fn pick(outputs: &[(String, bool)]) -> Result<usize> {
        // Simple heuristic: prefer the one whose tests passed.
        // If both pass, prefer the smaller diff (less invasive).
        if outputs.len() != 2 {
            anyhow::bail!("A/B judge expects exactly 2 outputs, got {}", outputs.len());
        }
        let (d0, t0) = &outputs[0];
        let (d1, t1) = &outputs[1];
        match (t0, t1) {
            (true, false) => Ok(0),
            (false, true) => Ok(1),
            (true, true) => {
                if d0.len() <= d1.len() {
                    Ok(0)
                } else {
                    Ok(1)
                }
            }
            (false, false) => {
                // Both fail: prefer shorter diff (less risky)
                if d0.len() <= d1.len() {
                    Ok(0)
                } else {
                    Ok(1)
                }
            }
        }
    }
}
