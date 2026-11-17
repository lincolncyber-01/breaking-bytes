//! Política pura: prioridade ponderada, reserva de urgência e cota móvel de 24 horas.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signals {
    pub impact: f64,
    pub novelty: f64,
    pub reliability: f64,
    pub reach: f64,
    pub urgency: f64,
}

impl Signals {
    pub fn score(self) -> Result<f64, PolicyError> {
        let values = [
            self.impact,
            self.novelty,
            self.reliability,
            self.reach,
            self.urgency,
        ];
        if values
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(PolicyError::Signals);
        }
        Ok(values
            .into_iter()
            .zip([0.30, 0.20, 0.20, 0.15, 0.15])
            .map(|(v, w)| v * w)
            .sum())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub daily_cap: u32,
    pub reserved: u32,
    pub breaking_score: f64,
    pub min_gap_seconds: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub remote_usage: u32,
    pub remote_limit: u32,
    pub local_published: u32,
    pub pending: u32,
    pub since_last_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Ready,
    Wait(u64),
    Capacity,
    LowScore,
}

impl Policy {
    pub fn validate(self) -> Result<Self, PolicyError> {
        if !(1..=80).contains(&self.daily_cap)
            || self.reserved >= self.daily_cap
            || !self.breaking_score.is_finite()
            || !(0.0..=1.0).contains(&self.breaking_score)
            || !(1..=86_400).contains(&self.min_gap_seconds)
        {
            return Err(PolicyError::Options);
        }
        Ok(self)
    }

    /// `pending` exclui a própria reserva quando a política é reavaliada antes do envio.
    pub fn decide(
        self,
        budget: Budget,
        score: f64,
        correction: bool,
    ) -> Result<Decision, PolicyError> {
        self.validate()?;
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(PolicyError::Signals);
        }
        let cap = self.daily_cap.min(budget.remote_limit);
        let used = budget
            .remote_usage
            .max(budget.local_published)
            .saturating_add(budget.pending);
        let urgent = correction || score >= self.breaking_score;
        if used >= cap || !urgent && used >= cap.saturating_sub(self.reserved) {
            return Ok(Decision::Capacity);
        }
        // Aumenta progressivamente a exigência depois de consumir metade da cota normal.
        let pressure = f64::from(used) / f64::from(cap.max(1));
        let minimum = 0.50 + ((pressure - 0.50).max(0.0) * 0.60);
        if !urgent && score < minimum {
            return Ok(Decision::LowScore);
        }
        if !urgent
            && let Some(elapsed) = budget.since_last_seconds
            && elapsed < self.min_gap_seconds
        {
            return Ok(Decision::Wait(self.min_gap_seconds - elapsed));
        }
        Ok(Decision::Ready)
    }
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("sinais editoriais devem ser números finitos entre zero e um")]
    Signals,
    #[error("política de publicação fora dos limites")]
    Options,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    fn policy() -> Policy {
        Policy {
            daily_cap: 40,
            reserved: 5,
            breaking_score: 0.8,
            min_gap_seconds: 90,
        }
    }
    fn budget(used: u32) -> Budget {
        Budget {
            remote_usage: used,
            remote_limit: 100,
            local_published: 0,
            pending: 0,
            since_last_seconds: Some(90),
        }
    }
    #[test]
    fn weights_reserve_and_gap() -> Result<(), PolicyError> {
        assert!(
            (Signals {
                impact: 1.0,
                novelty: 0.0,
                reliability: 0.0,
                reach: 0.0,
                urgency: 0.0
            }
            .score()?
                - 0.3)
                .abs()
                < 1e-12
        );
        assert_eq!(
            policy().decide(budget(35), 0.79, false)?,
            Decision::Capacity
        );
        assert_eq!(policy().decide(budget(35), 0.8, false)?, Decision::Ready);
        assert_eq!(policy().decide(budget(40), 1.0, true)?, Decision::Capacity);
        assert_eq!(
            policy().decide(
                Budget {
                    since_last_seconds: Some(1),
                    ..budget(0)
                },
                0.7,
                false
            )?,
            Decision::Wait(89)
        );
        assert_eq!(
            policy().decide(
                Budget {
                    remote_limit: 2,
                    ..budget(2)
                },
                1.0,
                true
            )?,
            Decision::Capacity
        );
        assert!(
            Signals {
                impact: f64::NAN,
                novelty: 1.0,
                reliability: 1.0,
                reach: 1.0,
                urgency: 1.0
            }
            .score()
            .is_err()
        );
        Ok(())
    }
    proptest! {
        #[test]
        fn weights_stay_in_range(a in 0.0f64..=1.0,b in 0.0f64..=1.0,c in 0.0f64..=1.0,d in 0.0f64..=1.0,e in 0.0f64..=1.0) {
            let score=Signals{impact:a,novelty:b,reliability:c,reach:d,urgency:e}.score()?;
            prop_assert!((0.0..=1.0+1e-12).contains(&score));
        }
        #[test]
        fn never_exceeds_effective_cap(remote in 0u32..200,local in 0u32..200,pending in 0u32..200,limit in 0u32..120) {
            let b=Budget{remote_usage:remote,remote_limit:limit,local_published:local,pending,since_last_seconds:None};
            if policy().decide(b,1.0,true)?==Decision::Ready {
                prop_assert!(remote.max(local).saturating_add(pending)<40.min(limit));
            }
        }
    }
}
