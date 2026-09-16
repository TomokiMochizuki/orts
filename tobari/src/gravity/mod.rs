//! Spherical-harmonic geopotential (EGM96 / EGM2008 / EIGEN-class fields).
//!
//! Two types, split the same way as `CssiData` / `CssiSpaceWeather`:
//!
//! - [`SphericalHarmonicCoefficients`] is the data — a fully normalized
//!   `C̄nm, S̄nm` set with its `GM`, reference radius and metadata, loaded
//!   from an ICGEM file or given explicitly. It is immutable and meant to be
//!   shared (`Arc`) between every evaluator that uses it.
//! - [`SphericalHarmonicField`] is an evaluator over a `degree × order` window
//!   of one such set. It owns the Legendre recursion table for that degree
//!   and nothing else; [`truncated`](SphericalHarmonicField::truncated) makes
//!   another window over the same coefficients without copying them.
//!
//! The field evaluates the **non-central** disturbing potential
//!
//! ```text
//! U(r, θ, λ) = (GM / r) Σ_{n=2}^{N} Σ_{m=0}^{min(n,M)} (a/r)^n P̄nm(cos θ) (C̄nm cos mλ + S̄nm sin mλ)
//! ```
//!
//! and its gradient `a = ∇U` (the acceleration a unit mass feels from
//! everything but the point-mass term), in the body-fixed frame the
//! coefficients are defined in. Degree 0 is the point mass — modelled
//! separately (orts: `PointMass`) so the two never double count — and degree
//! 1 vanishes for a geocentric field, so the sums start at `n = 2`. This is
//! the same split Orekit's `HolmesFeatherstoneAttractionModel` makes.
//!
//! The evaluator is frame-agnostic: it takes a body-fixed (ECEF) position in
//! km and returns km/s². Rotating the propagation state into the body frame
//! and back is the caller's job (orts: `SphericalHarmonicGravity<F>`).
//!
//! # Algorithm
//!
//! Holmes & Featherstone (2002) as implemented by Orekit: the Legendre
//! functions are evaluated with `u^m = sin^m θ` factored out
//! ([`legendre`]), summed over degree for each order, and the orders are
//! combined by Horner's rule in `u`. Two details differ from Orekit and make
//! the exact pole (`x = y = 0`) regular rather than `NaN`:
//!
//! - the θ-derivative is split into its `u^(m−1)` and `u^(m+1)` pieces and
//!   each is Horner-accumulated separately, so no `t/u` ever appears;
//! - the λ-derivative is accumulated as `(1/u)·∂U/∂λ` directly (its `m = 0`
//!   term is zero, so the sum starts at `u^(m−1)` with `m ≥ 1`).
//!
//! With `r̂ = (u cos λ, u sin λ, t)`, `θ̂ = (t cos λ, t sin λ, −u)`,
//! `λ̂ = (−sin λ, cos λ, 0)`:
//!
//! ```text
//! a = ∂U/∂r · r̂ + (1/r) ∂U/∂θ · θ̂ + (1/r) [(1/u) ∂U/∂λ] · λ̂
//! ```
//!
//! At the pole `cos λ, sin λ` are set to `(1, 0)`; every term that survives
//! there is independent of that choice.
//!
//! # Units and conventions
//!
//! km, km/s², km³/s² throughout (ICGEM files are SI and converted on load).
//! `U` is the positive disturbing potential, so `a = +∇U`. Coefficients are
//! fully (4π) normalized, the ICGEM `norm fully_normalized` convention. The
//! file's `tide_system` is recorded and **not** converted.
//!
//! # Cost
//!
//! `O(N·M)` per evaluation with `O(N)` scratch, allocated per call (four
//! small `Vec`s). The recursion coefficients (`O(N²)`) are precomputed once
//! per [`SphericalHarmonicField`]; the coefficient set itself is shared.

mod icgem;
mod legendre;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use nalgebra::Vector3;

use legendre::{HfRecursion, SCALE_UP, tri_index, tri_len};

pub use icgem::{IcgemError, TideSystem};

/// Highest degree a coefficient set may declare.
///
/// 2190 is the highest degree distributed by ICGEM (EGM2008 / EIGEN-6C4),
/// and the 2⁻⁹³⁰ scaling in [`legendre`] keeps `P̃nm(±1)` representable to
/// about degree 2700. The bound also keeps a malformed header from
/// requesting a gigabyte-scale coefficient allocation.
pub const MAX_DEGREE: usize = 2190;

// Used only on no_std (libm-backed `.sqrt()`); std uses the inherent method.
#[allow(unused_imports)]
use crate::math::F64Ext;

/// Why a coefficient set handed to
/// [`SphericalHarmonicCoefficients::from_normalized_coefficients`] was
/// rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum CoefficientError {
    /// `m > n` or `n > max_degree`.
    IndexOutOfRange { degree: usize, order: usize },
    /// A coefficient was NaN or infinite.
    NonFinite { degree: usize, order: usize },
    /// `gm` or `radius` was not a finite positive number, or `max_degree`
    /// exceeded [`MAX_DEGREE`].
    InvalidConstant(&'static str),
}

impl fmt::Display for CoefficientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndexOutOfRange { degree, order } => {
                write!(f, "coefficient (n={degree}, m={order}) out of range")
            }
            Self::NonFinite { degree, order } => {
                write!(f, "coefficient (n={degree}, m={order}) is not finite")
            }
            Self::InvalidConstant(name) => write!(f, "{name} must be finite and positive"),
        }
    }
}

impl core::error::Error for CoefficientError {}

/// Error from [`SphericalHarmonicCoefficients::from_icgem_file`].
#[cfg(feature = "std")]
#[derive(Debug)]
pub enum IcgemFileError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file was read but is not a usable static ICGEM gravity field.
    Parse(IcgemError),
}

#[cfg(feature = "std")]
impl fmt::Display for IcgemFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading ICGEM file: {e}"),
            Self::Parse(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for IcgemFileError {}

/// Why a `degree × order` window over a coefficient set could not be set up
/// ([`SphericalHarmonicField::new`] / [`truncated`](SphericalHarmonicField::truncated)).
///
/// Each of these is a request the evaluator cannot honour as stated. It does
/// not clamp instead: a simulation configured for 70×70 on a degree-36 file
/// would otherwise run at 36×36 and report nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TruncationError {
    /// `degree` exceeds the coefficient set's `max_degree`.
    DegreeUnavailable { requested: usize, available: usize },
    /// `order > degree` (there is no `C̄nm` with `m > n`).
    OrderExceedsDegree { degree: usize, order: usize },
    /// `degree < 2`: the field starts at degree 2 (degree 0 is the point mass,
    /// degree 1 vanishes), so such a field would evaluate to exactly zero.
    DegreeBelowTwo(usize),
}

impl fmt::Display for TruncationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DegreeUnavailable {
                requested,
                available,
            } => write!(
                f,
                "degree {requested} requested, but the coefficients stop at degree {available}"
            ),
            Self::OrderExceedsDegree { degree, order } => {
                write!(f, "order {order} exceeds degree {degree}")
            }
            Self::DegreeBelowTwo(d) => write!(
                f,
                "degree {d} has no non-central terms (the field starts at degree 2)"
            ),
        }
    }
}

impl core::error::Error for TruncationError {}

/// A static, fully normalized spherical-harmonic coefficient set with its
/// constants and source metadata.
///
/// Pure data: it evaluates nothing. Build a [`SphericalHarmonicField`] over
/// it to evaluate a `degree × order` window; several fields share one set
/// through `Arc`. It is deliberately not `Clone` (a set is shared, up to
/// 38 MB for degree 2190, never duplicated) and not `PartialEq` (bitwise
/// equality of the arrays is the wrong notion of "same model": the same
/// EGM2008 from two distributors can differ in the last bit).
pub struct SphericalHarmonicCoefficients {
    gm_km3_s2: f64,
    radius_km: f64,
    max_degree: usize,
    tide_system: TideSystem,
    model_name: Option<String>,
    /// C̄nm at `tri_index(n, m)`, `n ≤ max_degree`.
    c: Vec<f64>,
    /// S̄nm, same layout.
    s: Vec<f64>,
}

impl fmt::Debug for SphericalHarmonicCoefficients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SphericalHarmonicCoefficients")
            .field("gm_km3_s2", &self.gm_km3_s2)
            .field("radius_km", &self.radius_km)
            .field("max_degree", &self.max_degree)
            .field("tide_system", &self.tide_system)
            .field("model_name", &self.model_name)
            .finish_non_exhaustive()
    }
}

impl SphericalHarmonicCoefficients {
    /// Parse a static ICGEM `.gfc` text (see [`icgem`](self) rules and
    /// [`IcgemError`] for what is rejected).
    pub fn from_icgem(text: &str) -> Result<Self, IcgemError> {
        let p = icgem::parse(text)?;
        Ok(Self {
            gm_km3_s2: p.gm_km3_s2,
            radius_km: p.radius_km,
            max_degree: p.max_degree,
            tide_system: p.tide_system,
            model_name: p.model_name,
            c: p.c,
            s: p.s,
        })
    }

    /// Read and parse a static ICGEM `.gfc` file.
    ///
    /// Large official files (EGM2008 to degree 2190 is ~100 MB) parse in full;
    /// the [`SphericalHarmonicField`] built on top then selects the window the
    /// simulation needs.
    // TODO: stream-parse with a degree cut-off so a full EGM2008 file does not
    // allocate 2.4 M coefficients just to keep 70×70.
    #[cfg(feature = "std")]
    pub fn from_icgem_file(path: &std::path::Path) -> Result<Self, IcgemFileError> {
        let text = std::fs::read_to_string(path).map_err(IcgemFileError::Io)?;
        Self::from_icgem(&text).map_err(IcgemFileError::Parse)
    }

    /// Build a set from explicit fully normalized coefficients.
    ///
    /// `coefficients` are `(n, m, C̄nm, S̄nm)`; any `(n, m)` not listed is zero.
    /// Degree 0/1 entries are ignored by the evaluator (see module docs).
    pub fn from_normalized_coefficients(
        gm_km3_s2: f64,
        radius_km: f64,
        max_degree: usize,
        coefficients: &[(usize, usize, f64, f64)],
    ) -> Result<Self, CoefficientError> {
        if !(gm_km3_s2.is_finite() && gm_km3_s2 > 0.0) {
            return Err(CoefficientError::InvalidConstant("gm"));
        }
        if !(radius_km.is_finite() && radius_km > 0.0) {
            return Err(CoefficientError::InvalidConstant("radius"));
        }
        if max_degree > MAX_DEGREE {
            return Err(CoefficientError::InvalidConstant("max_degree"));
        }
        let len = tri_len(max_degree);
        let mut c = vec![0.0; len];
        let mut s = vec![0.0; len];
        for &(n, m, cnm, snm) in coefficients {
            if m > n || n > max_degree {
                return Err(CoefficientError::IndexOutOfRange {
                    degree: n,
                    order: m,
                });
            }
            if !(cnm.is_finite() && snm.is_finite()) {
                return Err(CoefficientError::NonFinite {
                    degree: n,
                    order: m,
                });
            }
            c[tri_index(n, m)] = cnm;
            s[tri_index(n, m)] = snm;
        }
        c[tri_index(0, 0)] = 1.0;
        Ok(Self {
            gm_km3_s2,
            radius_km,
            max_degree,
            tide_system: TideSystem::Unknown,
            model_name: None,
            c,
            s,
        })
    }

    /// Gravitational parameter of the set \[km³/s²\].
    ///
    /// Use this same value for the point-mass term. EGM2008's GM
    /// (398600.4415) differs from WGS-84's (398600.4418) by 7.5e-10 — only
    /// ~0.3 m/day along-track at LEO, but the central term and the harmonics
    /// are one model, so do not mix two GMs.
    pub fn gm(&self) -> f64 {
        self.gm_km3_s2
    }

    /// Reference radius the coefficients are scaled to \[km\] (not
    /// necessarily the WGS-84 equatorial radius).
    pub fn radius(&self) -> f64 {
        self.radius_km
    }

    /// Highest degree the set carries (every `m ≤ n ≤ max_degree` is stored).
    pub fn max_degree(&self) -> usize {
        self.max_degree
    }

    /// Permanent-tide convention declared by the source file.
    pub fn tide_system(&self) -> TideSystem {
        self.tide_system
    }

    /// `modelname` from the source file, if any.
    pub fn model_name(&self) -> Option<&str> {
        self.model_name.as_deref()
    }

    /// `(C̄nm, S̄nm)` for `m ≤ n ≤ max_degree`, else `None`.
    pub fn coefficient(&self, n: usize, m: usize) -> Option<(f64, f64)> {
        (m <= n && n <= self.max_degree).then(|| {
            let i = tri_index(n, m);
            (self.c[i], self.s[i])
        })
    }

    /// The unnormalized zonal coefficient `J2 = −√5 · C̄20`, for comparison
    /// with zonal-only models. `0` for a set that stops below degree 2 (it
    /// has no oblateness term to report).
    pub fn j2(&self) -> f64 {
        self.coefficient(2, 0)
            .map_or(0.0, |(c20, _)| -(5.0f64).sqrt() * c20)
    }
}

/// Evaluator of a `degree × order` window of a
/// [`SphericalHarmonicCoefficients`] set in its body-fixed frame.
///
/// Holds the coefficients by `Arc` and the Holmes–Featherstone recursion
/// table for `degree`; see the [module docs](self) for the model and its
/// conventions.
pub struct SphericalHarmonicField {
    coefficients: Arc<SphericalHarmonicCoefficients>,
    degree: usize,
    order: usize,
    recursion: HfRecursion,
}

impl fmt::Debug for SphericalHarmonicField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SphericalHarmonicField")
            .field("coefficients", &self.coefficients)
            .field("degree", &self.degree)
            .field("order", &self.order)
            .finish_non_exhaustive()
    }
}

impl SphericalHarmonicField {
    /// Evaluate `coefficients` up to `degree × order`.
    ///
    /// `degree` must lie in `2..=coefficients.max_degree()` and `order` in
    /// `0..=degree` (`0` is zonal only); anything else is a
    /// [`TruncationError`] rather than a clamp. Accepts the set by value or as
    /// an `Arc` — pass a cloned `Arc` to build several windows over one set.
    pub fn new(
        coefficients: impl Into<Arc<SphericalHarmonicCoefficients>>,
        degree: usize,
        order: usize,
    ) -> Result<Self, TruncationError> {
        let coefficients = coefficients.into();
        if degree < 2 {
            return Err(TruncationError::DegreeBelowTwo(degree));
        }
        if degree > coefficients.max_degree {
            return Err(TruncationError::DegreeUnavailable {
                requested: degree,
                available: coefficients.max_degree,
            });
        }
        if order > degree {
            return Err(TruncationError::OrderExceedsDegree { degree, order });
        }
        Ok(Self {
            coefficients,
            degree,
            order,
            recursion: HfRecursion::new(degree),
        })
    }

    /// Evaluate every coefficient the set carries (`max_degree × max_degree`).
    pub fn full(
        coefficients: impl Into<Arc<SphericalHarmonicCoefficients>>,
    ) -> Result<Self, TruncationError> {
        let coefficients = coefficients.into();
        let n = coefficients.max_degree;
        Self::new(coefficients, n, n)
    }

    /// Another window over the same coefficients, limited to `degree × order`.
    ///
    /// Shares the coefficient set (no copy); only the recursion table is
    /// rebuilt. The bounds are those of [`new`](Self::new) — this is not
    /// limited to this field's own window, so it can widen as well as narrow.
    pub fn truncated(&self, degree: usize, order: usize) -> Result<Self, TruncationError> {
        Self::new(Arc::clone(&self.coefficients), degree, order)
    }

    /// The coefficient set this field evaluates (clone the `Arc` to build
    /// another window over it).
    pub fn coefficients(&self) -> &Arc<SphericalHarmonicCoefficients> {
        &self.coefficients
    }

    /// Gravitational parameter \[km³/s²\]; see
    /// [`SphericalHarmonicCoefficients::gm`] for why the point mass must use
    /// this same value.
    pub fn gm(&self) -> f64 {
        self.coefficients.gm_km3_s2
    }

    /// Reference radius \[km\] ([`SphericalHarmonicCoefficients::radius`]).
    pub fn radius(&self) -> f64 {
        self.coefficients.radius_km
    }

    /// Highest degree evaluated (`≤ coefficients().max_degree()`).
    pub fn degree(&self) -> usize {
        self.degree
    }

    /// Highest order evaluated (`≤ degree`; `0` means zonal only).
    pub fn order(&self) -> usize {
        self.order
    }

    /// Non-central disturbing potential `U` \[km²/s²\] at a body-fixed
    /// position \[km\]. `a = ∇U`.
    pub fn potential_ecef(&self, position: &Vector3<f64>) -> f64 {
        self.evaluate(position).0
    }

    /// Non-central acceleration `∇U` \[km/s²\] at a body-fixed position \[km\].
    ///
    /// Non-finite or zero positions propagate to a non-finite result rather
    /// than being masked.
    pub fn acceleration_ecef(&self, position: &Vector3<f64>) -> Vector3<f64> {
        self.evaluate(position).1
    }

    /// Potential and acceleration together (they share every intermediate).
    fn evaluate(&self, position: &Vector3<f64>) -> (f64, Vector3<f64>) {
        let (x, y, z) = (position.x, position.y, position.z);
        let rho2 = x * x + y * y;
        let r2 = rho2 + z * z;
        let r = r2.sqrt();
        let rho = rho2.sqrt();
        let t = z / r; // cos θ (θ = colatitude)
        let u = rho / r; // sin θ
        // Longitude direction cosines; arbitrary at the pole, where every
        // surviving term is independent of them.
        let (cos_l, sin_l) = if rho > 0.0 {
            (x / rho, y / rho)
        } else {
            (1.0, 0.0)
        };

        let n_max = self.degree;
        let m_max = self.order;
        let (c, s) = (&self.coefficients.c, &self.coefficients.s);

        // (a/r)^n
        let a_over_r = self.coefficients.radius_km / r;
        let mut q = vec![1.0; n_max + 1];
        for n in 1..=n_max {
            q[n] = q[n - 1] * a_over_r;
        }
        // cos mλ, sin mλ by the angle-addition recurrence.
        let mut cos_m = vec![1.0; m_max + 1];
        let mut sin_m = vec![0.0; m_max + 1];
        for m in 1..=m_max {
            cos_m[m] = cos_m[m - 1] * cos_l - sin_m[m - 1] * sin_l;
            sin_m[m] = sin_m[m - 1] * cos_l + cos_m[m - 1] * sin_l;
        }

        // Legendre columns for the current order m and for m + 1 (the latter
        // feeds the θ-derivative). Both scaled by 2⁻⁹³⁰.
        let mut col = vec![0.0; n_max + 1];
        let mut col_next = vec![0.0; n_max + 1];
        if m_max < n_max {
            self.recursion.column(m_max + 1, t, &mut col_next);
        }

        // Horner accumulators over descending m (see module docs).
        let mut h_v = 0.0; // Σ u^m V_m            → U
        let mut h_r = 0.0; // Σ u^m Vr_m           → ∂U/∂r
        let mut h_e = 0.0; // Σ u^m VE_m           → ∂U/∂θ (u^(m+1) part)
        let mut g_t = 0.0; // Σ_{m≥1} u^(m−1) m t V_m → ∂U/∂θ (u^(m−1) part)
        let mut g_l = 0.0; // Σ_{m≥1} u^(m−1) m W_m   → (1/u) ∂U/∂λ

        for m in (0..=m_max).rev() {
            self.recursion.column(m, t, &mut col);

            let (mut a_c, mut a_s) = (0.0, 0.0); // Σ q P̃ C, Σ q P̃ S
            let (mut r_c, mut r_s) = (0.0, 0.0); // Σ n q P̃ C, …
            let (mut e_c, mut e_s) = (0.0, 0.0); // Σ q e P̃_{n,m+1} C, …
            for n in m.max(2)..=n_max {
                let i = tri_index(n, m);
                let (cnm, snm) = (c[i], s[i]);
                let qp = q[n] * col[n];
                let nqp = n as f64 * qp;
                // e_nn = 0 and col_next[n] is only defined for n > m.
                let qe = if n > m {
                    q[n] * self.recursion.e(n, m) * col_next[n]
                } else {
                    0.0
                };
                a_c += qp * cnm;
                a_s += qp * snm;
                r_c += nqp * cnm;
                r_s += nqp * snm;
                e_c += qe * cnm;
                e_s += qe * snm;
            }

            let (cm, sm) = (cos_m[m], sin_m[m]);
            let v = a_c * cm + a_s * sm;
            let w = a_s * cm - a_c * sm; // ∂V/∂(mλ)
            let vr = r_c * cm + r_s * sm;
            let ve = e_c * cm + e_s * sm;

            h_v = h_v * u + v;
            h_r = h_r * u + vr;
            h_e = h_e * u + ve;
            if m >= 1 {
                let mf = m as f64;
                g_t = g_t * u + mf * t * v;
                g_l = g_l * u + mf * w;
            }

            core::mem::swap(&mut col, &mut col_next);
        }

        let gm_over_r = self.coefficients.gm_km3_s2 / r;
        let potential = gm_over_r * h_v * SCALE_UP;
        let du_dr = -potential / r - gm_over_r / r * h_r * SCALE_UP;
        let du_dtheta = gm_over_r * (g_t - u * h_e) * SCALE_UP;
        let du_dlambda_over_u = gm_over_r * g_l * SCALE_UP;

        let r_hat = Vector3::new(u * cos_l, u * sin_l, t);
        let theta_hat = Vector3::new(t * cos_l, t * sin_l, -u);
        let lambda_hat = Vector3::new(-sin_l, cos_l, 0.0);
        let acceleration =
            du_dr * r_hat + (du_dtheta / r) * theta_hat + (du_dlambda_over_u / r) * lambda_hat;

        (potential, acceleration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const GM: f64 = 398600.4415;
    const A: f64 = 6378.1363;

    /// A field over every coefficient of an explicit set.
    fn field_from(
        max_degree: usize,
        coefficients: &[(usize, usize, f64, f64)],
    ) -> SphericalHarmonicField {
        let coefficients = SphericalHarmonicCoefficients::from_normalized_coefficients(
            GM,
            A,
            max_degree,
            coefficients,
        )
        .unwrap();
        SphericalHarmonicField::full(coefficients).unwrap()
    }

    fn single(n: usize, m: usize, c: f64, s: f64) -> SphericalHarmonicField {
        field_from(n.max(2), &[(n, m, c, s)])
    }

    /// Gradient of `P(x,y,z) / r^k` given `P` and `∇P` at `pos`.
    fn grad_over_rk(p: f64, grad_p: Vector3<f64>, k: f64, pos: &Vector3<f64>) -> Vector3<f64> {
        let r = pos.norm();
        grad_p / r.powf(k) - k * p * pos / r.powf(k + 2.0)
    }

    /// Closed forms of the degree-2/3 terms as polynomials over r^k, from
    /// P̄20 = √5(3t²−1)/2, P̄21 = √15 t u, P̄22 = (√15/2) u², P̄30 = √7(5t³−3t)/2
    /// with t = z/r, u cos λ = x/r, u sin λ = y/r.
    fn closed_form(n: usize, m: usize, c: f64, s: f64, pos: &Vector3<f64>) -> (f64, Vector3<f64>) {
        let (x, y, z) = (pos.x, pos.y, pos.z);
        let r2 = pos.norm_squared();
        match (n, m) {
            (2, 0) => {
                let k = GM * A * A * c * 5.0f64.sqrt() / 2.0;
                let p = k * (3.0 * z * z - r2);
                let g = k * Vector3::new(-2.0 * x, -2.0 * y, 4.0 * z);
                (p / r2.powf(2.5), grad_over_rk(p, g, 5.0, pos))
            }
            (2, 1) => {
                let k = GM * A * A * 15.0f64.sqrt();
                let p = k * z * (c * x + s * y);
                let g = k * Vector3::new(c * z, s * z, c * x + s * y);
                (p / r2.powf(2.5), grad_over_rk(p, g, 5.0, pos))
            }
            (2, 2) => {
                let k = GM * A * A * 15.0f64.sqrt() / 2.0;
                let p = k * (c * (x * x - y * y) + 2.0 * s * x * y);
                let g =
                    k * Vector3::new(2.0 * c * x + 2.0 * s * y, -2.0 * c * y + 2.0 * s * x, 0.0);
                (p / r2.powf(2.5), grad_over_rk(p, g, 5.0, pos))
            }
            (3, 0) => {
                let k = GM * A * A * A * c * 7.0f64.sqrt() / 2.0;
                let p = k * (5.0 * z * z * z - 3.0 * z * r2);
                let g = k * Vector3::new(
                    -6.0 * x * z,
                    -6.0 * y * z,
                    6.0 * z * z - 3.0 * (x * x + y * y),
                );
                (p / r2.powf(3.5), grad_over_rk(p, g, 7.0, pos))
            }
            _ => unreachable!(),
        }
    }

    fn sample_positions() -> Vec<Vector3<f64>> {
        vec![
            Vector3::new(6948.0, 0.0, 0.0),
            Vector3::new(4000.0, -3000.0, 5000.0),
            Vector3::new(-5000.0, 2000.0, -3000.0),
            Vector3::new(1.0, 1.0, 7000.0),      // near the north pole
            Vector3::new(0.0, 0.0, 7000.0),      // exactly at the north pole
            Vector3::new(0.0, 0.0, -6948.0),     // exactly at the south pole
            Vector3::new(42164.0, 100.0, -50.0), // GEO
            Vector3::new(0.0, 6900.0, 10.0),
        ]
    }

    fn assert_close(got: &Vector3<f64>, want: &Vector3<f64>, tol_rel: f64, scale: f64, ctx: &str) {
        let diff = (got - want).norm();
        assert!(
            diff <= tol_rel * scale,
            "{ctx}: diff {diff:e} > {tol_rel:e}·{scale:e} (got {got:?}, want {want:?})"
        );
    }

    /// Single-coefficient fields against hand-derived Cartesian closed forms —
    /// an oracle that shares no code with the recursion. Includes the exact
    /// poles, where the C̄21/S̄21 terms have a finite horizontal acceleration
    /// that a `t/u`-based implementation returns as NaN.
    #[test]
    fn single_coefficient_terms_match_closed_forms() {
        let cases = [
            (2, 0, -4.8e-4, 0.0),
            (2, 1, 1e-6, 0.0),
            (2, 1, 0.0, 1e-6),
            (2, 2, 2.4e-6, 0.0),
            (2, 2, 0.0, -1.4e-6),
            (2, 2, 2.4e-6, -1.4e-6),
            (3, 0, 9.6e-7, 0.0),
        ];
        for (n, m, c, s) in cases {
            let field = single(n, m, c, s);
            for pos in sample_positions() {
                let (u_want, a_want) = closed_form(n, m, c, s, &pos);
                let u_got = field.potential_ecef(&pos);
                let a_got = field.acceleration_ecef(&pos);
                // Scale: the point-mass acceleration / potential at r, so a
                // vanishing term (e.g. C̄22 at the pole) does not blow up a
                // relative tolerance.
                let r = pos.norm();
                assert_close(
                    &a_got,
                    &a_want,
                    1e-13,
                    GM / (r * r),
                    &alloc::format!("a n={n} m={m} pos={pos:?}"),
                );
                let du = (u_got - u_want).abs();
                assert!(
                    du <= 1e-13 * GM / r,
                    "U n={n} m={m} pos={pos:?}: {u_got} vs {u_want}"
                );
            }
        }
    }

    #[test]
    fn c21_gives_finite_horizontal_acceleration_at_the_pole() {
        let field = single(2, 1, 1e-6, 0.0);
        let r = 7000.0;
        let a = field.acceleration_ecef(&Vector3::new(0.0, 0.0, r));
        // ∇(√15 GM a² C x z / r⁵) at (0,0,r) = (√15 GM a² C / r⁴, 0, 0)
        let want = 15.0f64.sqrt() * GM * A * A * 1e-6 / r.powi(4);
        assert!(a.iter().all(|v| v.is_finite()), "{a:?}");
        assert!(
            (a.x - want).abs() <= 1e-13 * want.abs(),
            "{} vs {want}",
            a.x
        );
        assert!(
            a.y.abs() <= 1e-13 * want.abs() && a.z.abs() <= 1e-13 * want.abs(),
            "{a:?}"
        );
    }

    /// Deterministic pseudo-random coefficient set: every (n, m) non-zero,
    /// decaying like Kaula's rule so magnitudes are realistic.
    fn synthetic_coefficients(degree: usize) -> SphericalHarmonicCoefficients {
        let mut coeffs = Vec::new();
        for n in 2..=degree {
            for m in 0..=n {
                let k = 1e-5 / ((n * n) as f64);
                let phase = 1.3 * n as f64 + 0.7 * m as f64;
                coeffs.push((
                    n,
                    m,
                    k * phase.sin(),
                    if m == 0 { 0.0 } else { k * phase.cos() },
                ));
            }
        }
        SphericalHarmonicCoefficients::from_normalized_coefficients(GM, A, degree, &coeffs).unwrap()
    }

    /// A `degree × order` window over [`synthetic_coefficients`]`(degree)`.
    fn synthetic_field(degree: usize, order: usize) -> SphericalHarmonicField {
        SphericalHarmonicField::new(synthetic_coefficients(degree), degree, order).unwrap()
    }

    /// `a = ∇U` by central differences of the potential at two step sizes,
    /// both of which must agree to well below any plausible formula error
    /// (the O(h²) truncation and the round-off floor are each ~1e-10
    /// relative here). Supporting evidence for the full multi-order sum; the
    /// closed forms above are the independent oracle.
    #[test]
    fn acceleration_is_gradient_of_potential() {
        let field = synthetic_field(12, 12);
        for pos in sample_positions() {
            let a = field.acceleration_ecef(&pos);
            let mut errs = Vec::new();
            for h in [1e-2, 1e-3] {
                let mut fd = Vector3::zeros();
                for k in 0..3 {
                    let mut dp = Vector3::zeros();
                    dp[k] = h;
                    fd[k] = (field.potential_ecef(&(pos + dp)) - field.potential_ecef(&(pos - dp)))
                        / (2.0 * h);
                }
                errs.push((fd - a).norm());
            }
            let scale = a.norm();
            for (h, err) in [1e-2, 1e-3].iter().zip(&errs) {
                assert!(
                    *err <= 1e-8 * scale,
                    "pos={pos:?} h={h}: fd err {err:e} vs |a| {scale:e}"
                );
            }
        }
    }

    #[test]
    fn pole_is_the_limit_of_nearby_points() {
        let field = synthetic_field(20, 20);
        for z in [7000.0, -7000.0] {
            let at_pole = field.acceleration_ecef(&Vector3::new(0.0, 0.0, z));
            assert!(at_pole.iter().all(|v| v.is_finite()));
            for az in [0.0f64, 1.0, 2.5, 4.0] {
                let eps = 1e-6; // km
                let near = Vector3::new(eps * az.cos(), eps * az.sin(), z);
                let a_near = field.acceleration_ecef(&near);
                assert_close(
                    &a_near,
                    &at_pole,
                    1e-9,
                    at_pole.norm(),
                    &alloc::format!("z={z} az={az}"),
                );
            }
        }
    }

    #[test]
    fn non_finite_or_zero_position_yields_non_finite_output() {
        let field = synthetic_field(4, 4);
        for pos in [
            Vector3::new(f64::NAN, 0.0, 7000.0),
            Vector3::new(7000.0, f64::INFINITY, 0.0),
            Vector3::zeros(),
        ] {
            let a = field.acceleration_ecef(&pos);
            assert!(!a.iter().all(|v| v.is_finite()), "{pos:?} → {a:?}");
            assert!(!field.potential_ecef(&pos).is_finite());
        }
    }

    #[test]
    fn truncation_equals_zeroing_the_dropped_coefficients() {
        let full = synthetic_field(10, 10);
        let truncated = full.truncated(6, 3).unwrap();
        assert_eq!((truncated.degree(), truncated.order()), (6, 3));
        // The window shares the set rather than copying it.
        assert!(Arc::ptr_eq(truncated.coefficients(), full.coefficients()));
        let mut kept = Vec::new();
        for n in 2..=6 {
            for m in 0..=n.min(3) {
                let (c, s) = full.coefficients().coefficient(n, m).unwrap();
                kept.push((n, m, c, s));
            }
        }
        let zeroed = field_from(10, &kept);
        for pos in sample_positions() {
            let a = truncated.acceleration_ecef(&pos);
            assert_close(
                &a,
                &zeroed.acceleration_ecef(&pos),
                1e-14,
                a.norm(),
                "truncation",
            );
        }
    }

    /// A window the set cannot provide is an error, not a silent clamp: a
    /// 70×70 simulation on a degree-36 file must fail to configure rather than
    /// quietly run at 36×36.
    #[test]
    fn windows_outside_the_set_are_rejected() {
        let coefficients = Arc::new(synthetic_coefficients(10));
        let full = SphericalHarmonicField::full(Arc::clone(&coefficients)).unwrap();
        assert_eq!((full.degree(), full.order()), (10, 10));
        assert_eq!(
            full.truncated(11, 0).unwrap_err(),
            TruncationError::DegreeUnavailable {
                requested: 11,
                available: 10
            }
        );
        assert_eq!(
            SphericalHarmonicField::new(Arc::clone(&coefficients), 6, 7).unwrap_err(),
            TruncationError::OrderExceedsDegree {
                degree: 6,
                order: 7
            }
        );
        // A narrowed window can be widened again, up to the set's degree.
        let narrow = full.truncated(4, 0).unwrap();
        let widened = narrow.truncated(10, 10).unwrap();
        assert_eq!((widened.degree(), widened.order()), (10, 10));
    }

    #[test]
    fn zonal_only_truncation_ignores_tesseral_coefficients() {
        let full = synthetic_field(6, 6);
        let zonal = full.truncated(6, 0).unwrap();
        let mut zonal_coeffs = Vec::new();
        for n in 2..=6 {
            let (c, _) = full.coefficients().coefficient(n, 0).unwrap();
            zonal_coeffs.push((n, 0, c, 0.0));
        }
        let explicit = field_from(6, &zonal_coeffs);
        for pos in sample_positions() {
            let a = zonal.acceleration_ecef(&pos);
            assert_close(
                &a,
                &explicit.acceleration_ecef(&pos),
                1e-14,
                a.norm(),
                "zonal",
            );
        }
    }

    #[test]
    fn j2_is_minus_sqrt5_c20() {
        let c = SphericalHarmonicCoefficients::from_normalized_coefficients(
            GM,
            A,
            2,
            &[(2, 0, -4.84165143790815e-4, 0.0)],
        )
        .unwrap();
        assert!((c.j2() - 1.08262617385222e-3).abs() < 1e-15, "{}", c.j2());
    }

    /// A window below degree 2 has nothing to evaluate, so it is refused up
    /// front rather than built as a field that returns exactly zero.
    #[test]
    fn windows_below_degree_two_are_rejected() {
        let full = synthetic_field(6, 6);
        for (d, o) in [(0, 0), (1, 1), (1, 0)] {
            assert_eq!(
                full.truncated(d, o).unwrap_err(),
                TruncationError::DegreeBelowTwo(d)
            );
        }
        // A set that stops below degree 2 is valid data with nothing to
        // evaluate: `full` refuses it the same way.
        let degree_one =
            SphericalHarmonicCoefficients::from_normalized_coefficients(GM, A, 1, &[]).unwrap();
        assert_eq!(degree_one.j2(), 0.0);
        assert_eq!(degree_one.coefficient(2, 0), None);
        assert_eq!(
            SphericalHarmonicField::full(degree_one).unwrap_err(),
            TruncationError::DegreeBelowTwo(1)
        );
    }

    #[test]
    fn from_icgem_reads_header_and_coefficients_and_full_covers_them() {
        let text = "\
earth_gravity_constant 3.986004415E+14
radius 6378136.3
max_degree 2
norm fully_normalized
end_of_head
gfc 2 0 -4.8e-4 0.0
gfc 2 1 0.0 0.0
gfc 2 2 2.4e-6 -1.4e-6
";
        let c = SphericalHarmonicCoefficients::from_icgem(text).unwrap();
        assert_eq!(c.max_degree(), 2);
        assert_eq!(c.gm(), 398600.4415);
        assert_eq!(c.radius(), 6378.1363);
        assert_eq!(c.tide_system(), TideSystem::Unknown);
        assert_eq!(c.coefficient(2, 2), Some((2.4e-6, -1.4e-6)));
        assert_eq!(c.coefficient(3, 0), None);
        assert_eq!(c.coefficient(1, 2), None);
        let f = SphericalHarmonicField::full(c).unwrap();
        assert_eq!((f.degree(), f.order()), (2, 2));
        assert_eq!((f.gm(), f.radius()), (398600.4415, 6378.1363));
    }

    #[test]
    fn from_normalized_coefficients_validates_input() {
        let build = |gm, max_degree, coeffs: &[(usize, usize, f64, f64)]| {
            SphericalHarmonicCoefficients::from_normalized_coefficients(gm, A, max_degree, coeffs)
                .unwrap_err()
        };
        assert_eq!(
            build(GM, 2, &[(3, 0, 1.0, 0.0)]),
            CoefficientError::IndexOutOfRange {
                degree: 3,
                order: 0
            }
        );
        assert_eq!(
            build(GM, 2, &[(2, 0, f64::NAN, 0.0)]),
            CoefficientError::NonFinite {
                degree: 2,
                order: 0
            }
        );
        assert_eq!(build(-1.0, 2, &[]), CoefficientError::InvalidConstant("gm"));
        assert_eq!(
            build(GM, MAX_DEGREE + 1, &[]),
            CoefficientError::InvalidConstant("max_degree")
        );
    }

    /// Rotating the body about its pole by Δλ is the same as rotating the
    /// coefficients: C' = C cos mΔ − S sin mΔ, S' = C sin mΔ + S cos mΔ.
    fn rotated_coefficients(field: &SphericalHarmonicField, delta: f64) -> SphericalHarmonicField {
        let mut coeffs = Vec::new();
        for n in 2..=field.degree() {
            for m in 0..=n {
                let (c, s) = field.coefficients().coefficient(n, m).unwrap();
                let (sn, cs) = (m as f64 * delta).sin_cos();
                coeffs.push((n, m, c * cs - s * sn, c * sn + s * cs));
            }
        }
        field_from(field.degree(), &coeffs)
    }

    proptest! {
        /// `a'(R_Δ p) = R_Δ a(p)` where `a'` uses the rotated coefficients:
        /// pins the sign and indexing of the longitude-dependent terms (a
        /// swapped C/S or a wrong `sin mλ` sign breaks this, a zonal-only error
        /// does not — the closed forms cover those).
        #[test]
        fn rotation_about_the_pole_commutes_with_coefficient_rotation(
            x in -9000.0f64..9000.0, y in -9000.0f64..9000.0, z in -9000.0f64..9000.0,
            delta in -3.2f64..3.2,
        ) {
            let pos = Vector3::new(x, y, z);
            prop_assume!(pos.norm() > 6400.0);
            let field = synthetic_field(8, 8);
            let rotated = rotated_coefficients(&field, delta);
            let (sd, cd) = delta.sin_cos();
            let rot = nalgebra::Matrix3::new(cd, -sd, 0.0, sd, cd, 0.0, 0.0, 0.0, 1.0);
            let want = rot * field.acceleration_ecef(&pos);
            let got = rotated.acceleration_ecef(&(rot * pos));
            let scale = want.norm();
            prop_assert!((got - want).norm() <= 1e-11 * scale, "got {got:?} want {want:?}");
        }

        /// The evaluator is linear in the coefficients.
        #[test]
        fn acceleration_is_linear_in_coefficients(
            x in -9000.0f64..9000.0, y in -9000.0f64..9000.0, z in -9000.0f64..9000.0,
            alpha in -3.0f64..3.0,
        ) {
            let pos = Vector3::new(x, y, z);
            prop_assume!(pos.norm() > 6400.0);
            let field = synthetic_field(6, 6);
            let mut scaled = Vec::new();
            for n in 2..=6 {
                for m in 0..=n {
                    let (c, s) = field.coefficients().coefficient(n, m).unwrap();
                    scaled.push((n, m, alpha * c, alpha * s));
                }
            }
            let scaled = field_from(6, &scaled);
            let base = field.acceleration_ecef(&pos);
            let want = alpha * base;
            let got = scaled.acceleration_ecef(&pos);
            prop_assert!((got - want).norm() <= 1e-12 * base.norm() * alpha.abs().max(1.0));
        }
    }
}
