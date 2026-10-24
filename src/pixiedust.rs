// SPDX-License-Identifier: MIT
//! Particle behavior adapted from Neovide (MIT, Neovide Contributors).
//! No terminal, clock, Ratatui, or renderer dependencies in the simulation.
//! See LICENSES/neovide-MIT.txt and THIRD_PARTY_NOTICES.md.
pub mod graphics;
use crate::config::PixiedustConfig;
use std::f32::consts::{FRAC_PI_2, TAU};
pub const MAX_PARTICLES: usize = 96;
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub position: [f32; 2],
    pub velocity: [f32; 2],
    pub curl: f32,
    pub remaining: f32,
    pub size: u8,
}

pub struct Particles {
    particles: Vec<Particle>,
    rng: u64,
    remainder: f32,
    tokens: f32,
}
impl Default for Particles {
    fn default() -> Self {
        Self {
            particles: Vec::with_capacity(MAX_PARTICLES),
            rng: 0x853c49e6748fea9b,
            remainder: 0.0,
            tokens: 32.0,
        }
    }
}
impl Particles {
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }
    pub fn clear(&mut self) {
        self.particles.clear();
        self.remainder = 0.0;
        self.tokens = 32.0;
    }
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u32 << 24) as f32
    }
    pub fn advance(&mut self, dt: f32) {
        if !dt.is_finite() || dt < 0.0 {
            self.clear();
            return;
        }
        self.tokens = (self.tokens + dt * 128.0).min(32.0);
        if dt > 0.25 {
            self.particles.clear();
            return;
        }
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.remaining -= dt;
            if p.remaining <= f32::EPSILON * 4.0 {
                self.particles.swap_remove(i);
                continue;
            }
            // Exact integration of constant curl avoids frame-rate-dependent
            // drift. Neovide uses Euler translation followed by this rotation.
            let angle = p.curl * dt;
            let (sin, cos) = angle.sin_cos();
            let [x, y] = p.velocity;
            if p.curl.abs() > 0.0001 {
                p.position[0] += (x * sin + y * (cos - 1.0)) / p.curl;
                p.position[1] += (x * (1.0 - cos) + y * sin) / p.curl;
            } else {
                p.position[0] += x * dt;
                p.position[1] += y * dt;
            }
            p.velocity = [x * cos - y * sin, x * sin + y * cos];
            i += 1;
        }
    }
    pub fn emit(
        &mut self,
        from: [f32; 2],
        to: [f32; 2],
        cell_height: f32,
        config: PixiedustConfig,
    ) {
        if !from.into_iter().chain(to).all(f32::is_finite)
            || !cell_height.is_finite()
            || cell_height <= 0.0
            || !config.density.is_finite()
            || config.density <= 0.0
        {
            return;
        }
        let travel = [to[0] - from[0], to[1] - from[1]];
        let distance = travel[0].hypot(travel[1]) / cell_height;
        if distance > 25.0 {
            self.remainder = 0.0;
            return;
        }
        let desired = distance * config.density + self.remainder;
        self.remainder = desired.fract();
        let count = (desired as usize)
            .min(32)
            .min(self.tokens as usize)
            .min(MAX_PARTICLES - self.particles.len());
        self.tokens -= count as f32;
        for i in 0..count {
            // Stratification preserves Neovide's randomized trajectory emission
            // while avoiding clusters/gaps at moderate terminal frame rates.
            let t = (i as f32 + self.random()) / count as f32;
            let (sin, cos) = (self.random() * TAU).sin_cos();
            let curl = (self.random() - 0.5) * FRAC_PI_2;
            let size = (self.random() * 3.0) as u8;
            self.particles.push(Particle {
                position: [
                    from[0] + travel[0] * t,
                    from[1] + travel[1] * t + cell_height * 0.5,
                ],
                velocity: [cos * 15.0, (0.4 + sin.abs()) * 30.0],
                curl,
                remaining: ((i + 1) as f32 / count as f32) * f32::from(config.lifetime_ms) / 1000.0,
                size,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stratified_emission_scatter_and_expiration() {
        let mut p = Particles::default();
        let config = PixiedustConfig {
            density: 4.0,
            ..Default::default()
        };
        p.emit([10.0, 20.0], [110.0, 20.0], 20.0, config);
        assert_eq!(p.particles.len(), 20);
        for (i, particle) in p.particles.iter().enumerate() {
            assert!((10.0 + i as f32 * 5.0..15.0 + i as f32 * 5.0).contains(&particle.position[0]));
            assert_eq!(particle.position[1], 30.0);
            assert!(particle.velocity[1] >= 12.0);
            assert!(particle.velocity[0].abs() <= 15.0);
        }
        let before = p.particles.last().unwrap().position;
        p.advance(0.1);
        let last = p.particles.iter().find(|p| p.remaining > 0.39).unwrap();
        assert!(last.position[1] > before[1]);
        p.advance(0.6);
        assert!(p.particles.is_empty());
        assert_eq!(p.particles.capacity(), MAX_PARTICLES);
    }
    #[test]
    fn fractional_movement_density_and_hard_limits() {
        let config = PixiedustConfig::default();
        let mut p = Particles::default();
        for i in 0..100 {
            p.emit(
                [i as f32 * 0.2, 0.0],
                [(i + 1) as f32 * 0.2, 0.0],
                1.0,
                config,
            );
        }
        assert!((13..=14).contains(&p.particles.len()));
        p.clear();
        let config = PixiedustConfig {
            density: 4.0,
            lifetime_ms: 1500,
            ..config
        };
        for _ in 0..2000 {
            p.advance(0.001);
            p.emit([0.0, 0.0], [100.0, 0.0], 20.0, config);
            assert!(p.particles.len() <= MAX_PARTICLES);
        }
        assert_eq!(p.particles.capacity(), MAX_PARTICLES);
        p.clear();
        p.emit([0.0, 0.0], [100_000.0, 0.0], 20.0, config);
        assert!(p.particles.is_empty());
        p.emit([0.0, 0.0], [0.0, 0.0], 20.0, config);
        assert!(p.particles.is_empty());
    }
    #[test]
    fn curl_integration_is_independent_of_frame_rate_and_invalid_time_is_safe() {
        let mut a = Particles::default();
        a.emit(
            [0.0, 0.0],
            [100.0, 0.0],
            20.0,
            PixiedustConfig {
                density: 1.0,
                ..Default::default()
            },
        );
        let mut b = Particles::default();
        b.particles.clone_from(&a.particles);
        for _ in 0..10 {
            a.advance(0.01);
        }
        for _ in 0..20 {
            b.advance(0.005);
        }
        assert_eq!(a.particles.len(), b.particles.len());
        for (a, b) in a.particles.iter().zip(&b.particles) {
            assert!((a.position[0] - b.position[0]).abs() < 0.001);
            assert!((a.position[1] - b.position[1]).abs() < 0.001);
        }
        b.advance(f32::NAN);
        assert!(b.particles.is_empty());
        b.emit(
            [f32::INFINITY, 0.0],
            [0.0, 0.0],
            20.0,
            PixiedustConfig::default(),
        );
        assert!(b.particles.is_empty());
    }
    #[test]
    fn settings_validate_and_legacy_disable_controls_still_win() {
        use crate::config::{Config, CursorEffect};
        assert_eq!(
            Config::default().effects().cursor_effect,
            CursorEffect::Auto
        );
        for text in [
            "[visual]\ncursor_effect='off'",
            "[visual]\ncursor_animation=false\ncursor_effect='pixiedust'",
            "preset='minimal'",
        ] {
            assert!(!Config::parse(text).unwrap().effects().cursor);
        }
        for text in [
            "[visual]\ncursor_effect='invalid'",
            "[visual.pixiedust]\ndensity=nan",
            "[visual.pixiedust]\ndensity=5.0",
            "[visual.pixiedust]\nlifetime_ms=0",
            "[visual.pixiedust]\nfps=0",
        ] {
            assert!(Config::parse(text).is_err());
        }
        assert!(Config::parse(include_str!("../config.example.toml")).is_ok());
    }
}
