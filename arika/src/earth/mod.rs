//! Earth: physical constants, reference ellipsoid, geodetic coordinates,
//! Earth rotation models, and Earth Orientation Parameters.
//!
//! # Submodules
//!
//! - [`ellipsoid`] — WGS-84 reference ellipsoid constants
//! - [`geodetic`] — WGS-84 Cartesian ↔ geodetic conversions, [`Geodetic`] type
//! - [`topocentric`] — ground-site look angles ([`TopocentricSite`], [`LookAngles`])
//! - [`rotation`] — IAU 2009 WGCCRE rotation model (`EARTH` const)
//! - [`eop`] — Earth Orientation Parameters provider traits
//!   ([`Ut1Offset`](eop::Ut1Offset), [`PolarMotion`](eop::PolarMotion),
//!   [`NutationCorrections`](eop::NutationCorrections),
//!   [`LengthOfDay`](eop::LengthOfDay)) and [`NullEop`](eop::NullEop) placeholder
//! - [`iau2006`] — full IAU 2006 / 2000A_R06 CIO-based Earth rotation chain:
//!   the supporting math (angular units, fundamental arguments, precession /
//!   nutation polynomials) plus the `Rotation<Gcrs, Cirs>::iau2006` /
//!   `Rotation<Cirs, Tirs>::from_era` / `Rotation<Tirs, Itrs>::polar_motion`
//!   constructors that consume the [`eop`] traits.
//! - [`transform`] — per-frame Earth rotation pole
//!   ([`EarthRotationPole`](transform::EarthRotationPole)) and ECI ↔ ECEF
//!   transform ([`EarthFixedTransform`](transform::EarthFixedTransform), whose
//!   inputs are bundled as [`EarthOrientation`](transform::EarthOrientation))
//!   for `SimpleEci` / `Gcrs`, used by frame-aware force models

pub mod ellipsoid;
pub mod eop;
pub mod fk5;
pub mod geodetic;
pub mod iau2006;
pub mod mean_equinox;
pub mod rotation;
pub mod teme;
pub mod topocentric;
pub mod transform;

pub use ellipsoid::{WGS84_A, WGS84_B, WGS84_E2, WGS84_F};
#[cfg(feature = "alloc")]
pub use eop::GcrsEopStorage;
pub use eop::PositionEop;
pub use geodetic::{Geodetic, geodetic_altitude};
pub use topocentric::{LookAngles, TopocentricSite};
pub use transform::{EarthFixedTransform, EarthOrientation, EarthRotationPole};

// Physical constants

/// Earth gravitational parameter [km³/s²] (WGS-84).
pub const MU: f64 = 398600.4418;

/// Earth equatorial radius [km] (WGS-84).
pub const R: f64 = 6378.137;

/// Earth J2 zonal harmonic coefficient (WGS-84 / EGM96).
pub const J2: f64 = 1.08263e-3;

/// Earth J3 zonal harmonic coefficient (WGS-84 / EGM96).
pub const J3: f64 = -2.5356e-6;

/// Earth J4 zonal harmonic coefficient (WGS-84 / EGM96).
pub const J4: f64 = -1.6199e-6;

/// Nominal mean Earth angular velocity [rad/s] (WGS-84 defining parameter,
/// listed as the GRS80 nominal value in IERS Conventions 2010 Table 1.2).
///
/// A geodetic constant, alongside [`MU`] and [`R`]. For the angular velocity a
/// frame transform transports velocities with, use [`ERA_RATE`].
pub const OMEGA: f64 = 7.292_115e-5;

/// Rate the Earth Rotation Angle advances at [rad per UT1 second].
///
/// The derivative of the ERA expression (IAU 2000 Resolution B1.8; IERS
/// Conventions 2010 Eq. 5.14): `2π × 1.00273781191135448 / 86400`. Built from
/// the same [`ERA_TURNS_PER_UT1_DAY`](crate::epoch::ERA_TURNS_PER_UT1_DAY) the
/// angle itself is built from, so the two cannot drift apart.
///
/// **This is the ERA step alone.** The IAU 2006 rotation is `W·R·Q`, and its
/// full time derivative also carries the precession/nutation and polar-motion
/// rates `Q̇`/`Ẇ` plus a LOD correction, none of which this constant or
/// [`EarthFixedTransform`](crate::earth::EarthFixedTransform) include. What it
/// gives is the Earth-spin term, which is the one that dominates: the omitted
/// rates are sub-µrad/s. Transporting velocities with [`OMEGA`] instead left
/// even that term disagreeing with the `R` it differentiates, by
/// `1.47e-12 rad/s` — 0.0103 mm/s at 7000 km.
pub const ERA_RATE: f64 = core::f64::consts::TAU * crate::epoch::ERA_TURNS_PER_UT1_DAY / 86_400.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mu_is_positive() {
        assert!(MU > 0.0);
    }

    #[test]
    fn r_is_positive() {
        assert!(R > 0.0);
    }

    /// `OMEGA` is a published figure, and no implementation code reads it any
    /// more — the transforms and the drag models take [`ERA_RATE`]. Nothing
    /// else in the suite would notice it drifting, so it is pinned here.
    #[test]
    fn omega_is_the_wgs84_defining_value() {
        // NGA WGS-84: 7292115 × 10⁻¹¹ rad/s, the same figure IERS Conventions
        // 2010 Table 1.2 lists as the GRS80 nominal mean angular velocity.
        assert_eq!(OMEGA, 7_292_115e-11);
    }

    /// `ERA_RATE` has to be the slope of the angle the crate actually computes.
    /// Repeating the coefficient here would only restate the definition, and
    /// would not notice the two copies drifting — which is the shape of the
    /// mix-up this constant exists to end. So the rate is compared against a
    /// difference of [`Ut1Epoch::era`](crate::epoch::Ut1Epoch::era) instead.
    #[test]
    fn era_rate_is_the_slope_of_the_era_it_belongs_to() {
        use crate::epoch::Ut1Epoch;

        // A whole UT1 day, so the difference is the per-day advance. The ERA
        // wraps at 2π, and one day is 1.0027 turns, so the wrap is added back.
        let jd = 2_460_390.0;
        let advance = Ut1Epoch::from_jd_ut1(jd + 1.0).era() - Ut1Epoch::from_jd_ut1(jd).era()
            + core::f64::consts::TAU;
        let from_rate = ERA_RATE * 86_400.0;
        // `era()` wraps a value of ~5.6e4 rad, where f64 spacing is 7.3e-12,
        // and two of those are differenced: 2 ulps. Measured residual 2.4e-12.
        assert!(
            (advance - from_rate).abs() < 1.5e-11,
            "the rate is not the angle's slope: {advance:e} vs {from_rate:e}"
        );
    }

    /// The two are close enough that swapping them passes most numerical
    /// checks, which is how the mix-up survived: measured, they agree to 7.7
    /// significant digits and differ by 1.47e-12 rad/s, 2.01e-8 relative.
    #[test]
    fn the_two_rates_agree_to_seven_digits() {
        let gap = (ERA_RATE - OMEGA).abs();
        assert!(
            (1.46e-12..1.48e-12).contains(&gap),
            "expected a ~1.47e-12 rad/s gap, got {gap:e}"
        );
        let relative = gap / OMEGA;
        assert!(
            (2.0e-8..2.1e-8).contains(&relative),
            "expected a ~2.01e-8 relative gap, got {relative:e}"
        );
    }

    #[test]
    fn surface_gravity_approximate() {
        // g ≈ μ/R² ≈ 9.798e-3 km/s² ≈ 9.798 m/s²
        let g = MU / (R * R);
        assert!((g - 9.798e-3).abs() < 0.01e-3);
    }

    #[test]
    fn j2_is_positive() {
        assert!(J2 > 0.0);
    }

    #[test]
    fn j3_is_negative() {
        assert!(J3 < 0.0);
    }

    #[test]
    fn j4_is_negative() {
        assert!(J4 < 0.0);
    }

    #[test]
    fn j2_dominates_higher_harmonics() {
        assert!(J2 > J3.abs());
        assert!(J3.abs() > J4.abs());
    }
}
