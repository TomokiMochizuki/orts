//! ICGEM `.gfc` gravity-field file parser (static coefficients only).
//!
//! Format reference: ICGEM, "The ICGEM-format" (2023),
//! <https://icgem.gfz-potsdam.de/docs/ICGEM-Format-2023.pdf>. A file is a
//! free-form header of `keyword value` lines ended by `end_of_head`, followed
//! by data records. This parser accepts the static record kind
//!
//! ```text
//! gfc  n  m  C̄nm  S̄nm  [σC  σS]
//! ```
//!
//! and **rejects** the time-variable kinds (`gfct`, `trnd`/`dot`, `asin`,
//! `acos`) with [`IcgemParseError::TimeVariableUnsupported`]: their meaning
//! depends on a reference epoch and validity interval, so reading `gfct` as a
//! static coefficient would silently fabricate a static model out of a
//! time-variable one. Static models (EGM96, EGM2008, EIGEN-6C4, GOCO06s, …)
//! contain `gfc` records only.
//!
//! # Strictness
//!
//! Header lines are metadata and record lines are data, and the two are
//! treated differently on purpose:
//!
//! - an **unknown header keyword is ignored** (ICGEM files carry free-form
//!   provenance lines, and a new informational key must not break loading),
//!   but a **known header keyword declared twice is an error** — the parser
//!   sets no defaults for the keys it needs, so it cannot pick one of two
//!   declarations either;
//! - an **unknown record kind is an error** (data the parser cannot interpret
//!   is not something it may skip);
//! - values the parser *interprets* (`norm`, `errors`, `max_degree`, the
//!   constants) must be present and valid; values it only *records*
//!   (`tide_system`, `modelname`) accept anything, with an unrecognised
//!   `tide_system` spelling read as [`TideSystem::Unknown`] — the same as an
//!   absent line — since nothing downstream converts between systems.
//!
//! `product_type` is checked when present but not required: ICGEM 2023
//! declares it, but it names the *product* rather than anything the parser
//! interprets, and older `.gfc` files in circulation omit it while being
//! perfectly usable gravity fields.
//!
//! # Streaming
//!
//! [`Parser`] consumes one line at a time, so a caller can stop reading as
//! soon as [`Parser::is_complete`] reports that every `(n, m)` with
//! `n ≤ max_degree` — degrees 0 and 1 included, so their value checks in
//! [`Parser::finish`] still see them — has been read. The official EGM2008
//! file is ~132 MB of text for degree 2190; a caller that wants 70×70 reads
//! the header and the first ~2600 records and never allocates the other
//! 2.4 million. A file that omits the optional degree-0/1 records never
//! reports completion and is simply read to the end. Records above the
//! requested degree that arrive earlier (an unsorted file) are index-checked
//! and skipped. Lines after the completion point are, by construction, not
//! examined.
//!
//! Units in the file are SI (`m³/s²`, `m`); the parsed constants are in km.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use super::MAX_DEGREE;
use super::legendre::{tri_index, tri_len};

/// Permanent-tide convention of the C̄20 coefficient, as declared by the file.
///
/// The parser records this and the evaluator does **not** convert between
/// systems. A ~4e-9 difference in C̄20 separates `tide_free` from
/// `zero_tide`; whether that matters depends on which other tide models the
/// caller adds, so the decision is left to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TideSystem {
    /// Permanent tide removed entirely (e.g. EGM2008 as distributed).
    TideFree,
    /// Direct permanent tide removed, indirect (deformation) part kept.
    ZeroTide,
    /// Permanent tide fully included.
    MeanTide,
    /// Header said `unknown`, used a spelling this parser does not know, or
    /// had no `tide_system` line.
    Unknown,
}

/// Why an ICGEM text could not be parsed into
/// [`SphericalHarmonicCoefficients`](super::SphericalHarmonicCoefficients).
///
/// Reading a *file* adds I/O failures on top; that is
/// [`IcgemFileError`](super::IcgemFileError).
#[derive(Debug, Clone, PartialEq)]
pub enum IcgemParseError {
    /// `end_of_head` never appeared.
    MissingEndOfHead,
    /// A required header keyword is absent.
    MissingHeader(&'static str),
    /// A header keyword the parser uses appeared twice.
    DuplicateHeader(&'static str),
    /// A header value did not parse (`keyword`, `value`).
    InvalidHeader(&'static str, String),
    /// `product_type` was present but not `gravity_field`.
    NotAGravityField(String),
    /// `norm` was something other than `fully_normalized`.
    UnsupportedNormalization(String),
    /// The caller asked for a degree above the file's `max_degree`.
    DegreeUnavailable { requested: usize, available: usize },
    /// A time-variable record kind (`gfct`, `trnd`, `dot`, `asin`, `acos`).
    TimeVariableUnsupported { line: usize, kind: String },
    /// A data record kind this parser does not know.
    UnknownRecord { line: usize, kind: String },
    /// A `gfc` record with the wrong number of columns or an unparsable field.
    MalformedRecord { line: usize, reason: String },
    /// `n > max_degree` or `m > n`.
    IndexOutOfRange {
        line: usize,
        degree: usize,
        order: usize,
    },
    /// The same `(n, m)` appeared twice.
    DuplicateCoefficient {
        line: usize,
        degree: usize,
        order: usize,
    },
    /// A coefficient value was NaN or infinite.
    NonFiniteCoefficient {
        line: usize,
        degree: usize,
        order: usize,
    },
    /// `C̄00` was present and not 1 (the point-mass term is modelled separately).
    UnexpectedC00(f64),
    /// A degree-1 coefficient was non-zero: that encodes an offset between the
    /// coordinate origin and the centre of mass, which this evaluator does not
    /// model (it starts at degree 2).
    NonZeroDegreeOne { degree: usize, order: usize },
    /// A coefficient with `2 ≤ n ≤ max_degree` never appeared.
    MissingCoefficient { degree: usize, order: usize },
}

impl fmt::Display for IcgemParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEndOfHead => write!(f, "ICGEM: no `end_of_head` line"),
            Self::MissingHeader(k) => write!(f, "ICGEM: missing header `{k}`"),
            Self::DuplicateHeader(k) => write!(f, "ICGEM: header `{k}` declared twice"),
            Self::InvalidHeader(k, v) => write!(f, "ICGEM: header `{k}` has invalid value `{v}`"),
            Self::NotAGravityField(v) => {
                write!(f, "ICGEM: product_type is `{v}`, expected `gravity_field`")
            }
            Self::UnsupportedNormalization(v) => {
                write!(f, "ICGEM: norm `{v}` unsupported (only `fully_normalized`)")
            }
            Self::DegreeUnavailable {
                requested,
                available,
            } => write!(
                f,
                "ICGEM: degree {requested} requested, but the file carries max_degree {available}"
            ),
            Self::TimeVariableUnsupported { line, kind } => write!(
                f,
                "ICGEM line {line}: time-variable record `{kind}` unsupported (static `gfc` only)"
            ),
            Self::UnknownRecord { line, kind } => {
                write!(f, "ICGEM line {line}: unknown record kind `{kind}`")
            }
            Self::MalformedRecord { line, reason } => {
                write!(f, "ICGEM line {line}: malformed record: {reason}")
            }
            Self::IndexOutOfRange {
                line,
                degree,
                order,
            } => write!(
                f,
                "ICGEM line {line}: (n={degree}, m={order}) outside max_degree / m ≤ n"
            ),
            Self::DuplicateCoefficient {
                line,
                degree,
                order,
            } => write!(
                f,
                "ICGEM line {line}: duplicate coefficient (n={degree}, m={order})"
            ),
            Self::NonFiniteCoefficient {
                line,
                degree,
                order,
            } => write!(
                f,
                "ICGEM line {line}: non-finite coefficient (n={degree}, m={order})"
            ),
            Self::UnexpectedC00(v) => write!(f, "ICGEM: C00 = {v}, expected 1"),
            Self::NonZeroDegreeOne { degree, order } => write!(
                f,
                "ICGEM: non-zero degree-1 coefficient (n={degree}, m={order}) — origin offsets are not modelled"
            ),
            Self::MissingCoefficient { degree, order } => {
                write!(f, "ICGEM: coefficient (n={degree}, m={order}) missing")
            }
        }
    }
}

impl core::error::Error for IcgemParseError {}

/// Parsed contents of a static ICGEM file, SI units converted to km, cut at
/// the requested degree.
#[derive(Debug, PartialEq)]
pub(super) struct ParsedIcgem {
    pub gm_km3_s2: f64,
    pub radius_km: f64,
    /// The highest degree carried: the caller's cap, or the file's
    /// `max_degree` when no cap was given.
    pub max_degree: usize,
    pub tide_system: TideSystem,
    pub model_name: Option<String>,
    /// C̄nm at [`tri_index`]`(n, m)`, `n ≤ max_degree`.
    pub c: Vec<f64>,
    /// S̄nm, same layout.
    pub s: Vec<f64>,
}

/// Parse a Fortran-style float (`0.484D-03` as well as `4.84e-4`).
fn parse_f64(s: &str) -> Option<f64> {
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }
    let fixed: String = s
        .chars()
        .map(|ch| match ch {
            'D' | 'd' => 'e',
            other => other,
        })
        .collect();
    fixed.parse::<f64>().ok()
}

fn parse_index(s: &str) -> Option<usize> {
    s.parse::<usize>().ok()
}

/// Header values collected before `end_of_head`.
#[derive(Default)]
struct Header {
    product_type: Option<String>,
    gm: Option<f64>,
    radius: Option<f64>,
    max_degree: Option<usize>,
    norm: Option<String>,
    errors: Option<String>,
    tide_system: Option<TideSystem>,
    model_name: Option<String>,
}

/// Store `value` under `slot`, refusing a second declaration.
fn once<T>(slot: &mut Option<T>, key: &'static str, value: T) -> Result<(), IcgemParseError> {
    if slot.is_some() {
        return Err(IcgemParseError::DuplicateHeader(key));
    }
    *slot = Some(value);
    Ok(())
}

fn required<'a>(value: Option<&'a str>, key: &'static str) -> Result<&'a str, IcgemParseError> {
    value.ok_or(IcgemParseError::MissingHeader(key))
}

/// Header resolved at `end_of_head`, plus the record accumulators.
struct Records {
    gm: f64,
    radius: f64,
    /// The file's own `max_degree`.
    file_max_degree: usize,
    /// The degree the caller asked for (`≤ file_max_degree`).
    cap: usize,
    tide_system: TideSystem,
    model_name: Option<String>,
    /// Number of σ columns each record carries, from the `errors` header.
    error_columns: usize,
    /// What `errors` said, for the column-count message.
    errors_declared: String,
    c: Vec<f64>,
    s: Vec<f64>,
    seen: Vec<bool>,
    /// How many `(n, m)` with `n ≤ cap` have been seen (degree 0/1 included,
    /// so [`Parser::is_complete`] cannot fire before their checks see them).
    slots_seen: usize,
}

enum State {
    Header(Header),
    Records(Records),
}

/// Line-at-a-time ICGEM parser.
///
/// Feed every line in file order; the parser tracks the line number for its
/// messages. Once [`is_complete`](Self::is_complete) is true the remaining
/// records lie above the requested degree and the caller may stop early;
/// [`finish`](Self::finish) then runs the checks that need the whole set.
pub(super) struct Parser {
    state: State,
    line_no: usize,
    /// Highest degree the caller wants; `None` for the whole file.
    requested: Option<usize>,
}

impl Parser {
    pub(super) fn new(max_degree: Option<usize>) -> Self {
        Self {
            state: State::Header(Header::default()),
            line_no: 0,
            requested: max_degree,
        }
    }

    /// Every `(n, m)` with `n ≤ max_degree` has been seen, degrees 0 and 1
    /// included.
    ///
    /// Records above the cap are skipped anyway, so a caller reading a large
    /// file may stop feeding lines here. A file that omits the optional
    /// degree-0/1 records never reaches this state and is read to the end.
    pub(super) fn is_complete(&self) -> bool {
        match &self.state {
            State::Header(_) => false,
            State::Records(r) => r.slots_seen == r.seen.len(),
        }
    }

    pub(super) fn feed_line(&mut self, line: &str) -> Result<(), IcgemParseError> {
        self.line_no += 1;
        match &mut self.state {
            State::Header(header) => {
                if let Some(records) = Self::header_line(header, line, self.requested)? {
                    self.state = State::Records(records);
                }
                Ok(())
            }
            State::Records(records) => Self::record_line(records, line, self.line_no),
        }
    }

    /// One header line. Returns the resolved header at `end_of_head`.
    fn header_line(
        h: &mut Header,
        line: &str,
        requested: Option<usize>,
    ) -> Result<Option<Records>, IcgemParseError> {
        let mut tokens = line.split_whitespace();
        let Some(key) = tokens.next() else {
            return Ok(None);
        };
        let value = tokens.next();

        match key {
            "end_of_head" => return Self::resolve(h, requested).map(Some),
            "product_type" => once(
                &mut h.product_type,
                "product_type",
                required(value, "product_type")?.to_string(),
            )?,
            // Model names may contain spaces ("EIGEN-6C4 (static part)"), so
            // keep the whole remainder of the line, not the first token.
            "modelname" => {
                let name = line
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim().to_string())
                    .filter(|v| !v.is_empty());
                if let Some(name) = name {
                    once(&mut h.model_name, "modelname", name)?;
                }
            }
            "radius" => {
                let v = required(value, "radius")?;
                let r = parse_f64(v)
                    .ok_or_else(|| IcgemParseError::InvalidHeader("radius", v.to_string()))?;
                once(&mut h.radius, "radius", r)?;
            }
            "max_degree" => {
                let v = required(value, "max_degree")?;
                let d = parse_index(v)
                    .ok_or_else(|| IcgemParseError::InvalidHeader("max_degree", v.to_string()))?;
                once(&mut h.max_degree, "max_degree", d)?;
            }
            "norm" => once(&mut h.norm, "norm", required(value, "norm")?.to_string())?,
            "errors" => once(
                &mut h.errors,
                "errors",
                required(value, "errors")?.to_string(),
            )?,
            // Recorded, never interpreted: a spelling this parser does not know
            // is `Unknown`, the same as an absent line (see module docs).
            "tide_system" => once(
                &mut h.tide_system,
                "tide_system",
                match value {
                    Some("tide_free") => TideSystem::TideFree,
                    Some("zero_tide") => TideSystem::ZeroTide,
                    Some("mean_tide") => TideSystem::MeanTide,
                    _ => TideSystem::Unknown,
                },
            )?,
            // Non-Earth bodies use e.g. `gravity_constant`; accept any suffix
            // match like Orekit does.
            k if k.ends_with("gravity_constant") => {
                let v = required(value, "earth_gravity_constant")?;
                let gm = parse_f64(v).ok_or_else(|| {
                    IcgemParseError::InvalidHeader("earth_gravity_constant", v.to_string())
                })?;
                once(&mut h.gm, "earth_gravity_constant", gm)?;
            }
            _ => {}
        }
        Ok(None)
    }

    /// Validate the header at `end_of_head` and size the coefficient arrays.
    fn resolve(h: &mut Header, requested: Option<usize>) -> Result<Records, IcgemParseError> {
        if let Some(pt) = &h.product_type
            && pt != "gravity_field"
        {
            return Err(IcgemParseError::NotAGravityField(pt.clone()));
        }
        // The normalization changes the scale of every coefficient, so a file
        // that does not state it cannot be read safely: no default.
        match h.norm.as_deref() {
            Some("fully_normalized") => {}
            Some(other) => {
                return Err(IcgemParseError::UnsupportedNormalization(other.to_string()));
            }
            None => return Err(IcgemParseError::MissingHeader("norm")),
        }
        let (error_columns, errors_declared) = match h.errors.as_deref() {
            None => (0, "no (absent)".to_string()),
            Some("no") => (0, "no".to_string()),
            Some(v @ ("calibrated" | "formal")) => (2, v.to_string()),
            Some(v @ "calibrated_and_formal") => (4, v.to_string()),
            Some(other) => {
                return Err(IcgemParseError::InvalidHeader("errors", other.to_string()));
            }
        };
        let gm =
            h.gm.ok_or(IcgemParseError::MissingHeader("earth_gravity_constant"))?;
        let radius = h.radius.ok_or(IcgemParseError::MissingHeader("radius"))?;
        let file_max_degree = h
            .max_degree
            .ok_or(IcgemParseError::MissingHeader("max_degree"))?;
        if file_max_degree > MAX_DEGREE {
            return Err(IcgemParseError::InvalidHeader(
                "max_degree",
                alloc::format!("{file_max_degree} (limit {MAX_DEGREE})"),
            ));
        }
        if !(gm.is_finite() && gm > 0.0) {
            return Err(IcgemParseError::InvalidHeader(
                "earth_gravity_constant",
                gm.to_string(),
            ));
        }
        if !(radius.is_finite() && radius > 0.0) {
            return Err(IcgemParseError::InvalidHeader("radius", radius.to_string()));
        }
        let cap = match requested {
            Some(d) if d > file_max_degree => {
                return Err(IcgemParseError::DegreeUnavailable {
                    requested: d,
                    available: file_max_degree,
                });
            }
            Some(d) => d,
            None => file_max_degree,
        };

        let len = tri_len(cap);
        Ok(Records {
            gm,
            radius,
            file_max_degree,
            cap,
            tide_system: h.tide_system.unwrap_or(TideSystem::Unknown),
            model_name: h.model_name.take(),
            error_columns,
            errors_declared,
            c: vec![0.0; len],
            s: vec![0.0; len],
            seen: vec![false; len],
            slots_seen: 0,
        })
    }

    /// One data record.
    fn record_line(r: &mut Records, line: &str, line_no: usize) -> Result<(), IcgemParseError> {
        let mut tokens = line.split_whitespace();
        let Some(kind) = tokens.next() else {
            return Ok(());
        };
        match kind {
            "gfc" => {}
            "gfct" | "trnd" | "dot" | "asin" | "acos" => {
                return Err(IcgemParseError::TimeVariableUnsupported {
                    line: line_no,
                    kind: kind.to_string(),
                });
            }
            other => {
                return Err(IcgemParseError::UnknownRecord {
                    line: line_no,
                    kind: other.to_string(),
                });
            }
        }
        let fields: Vec<&str> = tokens.collect();
        let expected = 4 + r.error_columns;
        if fields.len() != expected {
            let sigma = match r.error_columns {
                0 => "",
                2 => ", σC, σS",
                _ => ", σC, σS, σC, σS",
            };
            return Err(IcgemParseError::MalformedRecord {
                line: line_no,
                reason: alloc::format!(
                    "expected {expected} columns after `gfc` (n, m, C, S{sigma}; the header \
                     declares `errors {}`), found {}",
                    r.errors_declared,
                    fields.len()
                ),
            });
        }
        let n = parse_index(fields[0]).ok_or_else(|| IcgemParseError::MalformedRecord {
            line: line_no,
            reason: alloc::format!("degree `{}` is not an integer", fields[0]),
        })?;
        let m = parse_index(fields[1]).ok_or_else(|| IcgemParseError::MalformedRecord {
            line: line_no,
            reason: alloc::format!("order `{}` is not an integer", fields[1]),
        })?;
        if n > r.file_max_degree || m > n {
            return Err(IcgemParseError::IndexOutOfRange {
                line: line_no,
                degree: n,
                order: m,
            });
        }
        if n > r.cap {
            // Above the requested degree: valid, but not wanted.
            return Ok(());
        }
        let cnm = parse_f64(fields[2]).ok_or_else(|| IcgemParseError::MalformedRecord {
            line: line_no,
            reason: alloc::format!("C `{}` is not a number", fields[2]),
        })?;
        let snm = parse_f64(fields[3]).ok_or_else(|| IcgemParseError::MalformedRecord {
            line: line_no,
            reason: alloc::format!("S `{}` is not a number", fields[3]),
        })?;
        if !(cnm.is_finite() && snm.is_finite()) {
            return Err(IcgemParseError::NonFiniteCoefficient {
                line: line_no,
                degree: n,
                order: m,
            });
        }
        let i = tri_index(n, m);
        if r.seen[i] {
            return Err(IcgemParseError::DuplicateCoefficient {
                line: line_no,
                degree: n,
                order: m,
            });
        }
        r.seen[i] = true;
        r.c[i] = cnm;
        r.s[i] = snm;
        r.slots_seen += 1;
        Ok(())
    }

    /// Run the whole-set checks and hand over the coefficients.
    pub(super) fn finish(self) -> Result<ParsedIcgem, IcgemParseError> {
        let mut r = match self.state {
            State::Header(_) => return Err(IcgemParseError::MissingEndOfHead),
            State::Records(r) => r,
        };
        let i00 = tri_index(0, 0);
        if r.seen[i00] && (r.c[i00] - 1.0).abs() > 1e-9 {
            return Err(IcgemParseError::UnexpectedC00(r.c[i00]));
        }
        if r.cap >= 1 {
            for m in 0..=1 {
                let i = tri_index(1, m);
                if r.c[i] != 0.0 || r.s[i] != 0.0 {
                    return Err(IcgemParseError::NonZeroDegreeOne {
                        degree: 1,
                        order: m,
                    });
                }
            }
        }
        for n in 2..=r.cap {
            for m in 0..=n {
                if !r.seen[tri_index(n, m)] {
                    return Err(IcgemParseError::MissingCoefficient {
                        degree: n,
                        order: m,
                    });
                }
            }
        }
        // The degree-0/1 slots are exactly 1 / 0 whether or not the file
        // spelled them out.
        r.c[i00] = 1.0;

        Ok(ParsedIcgem {
            gm_km3_s2: r.gm / 1e9,
            radius_km: r.radius / 1e3,
            max_degree: r.cap,
            tide_system: r.tide_system,
            model_name: r.model_name,
            c: r.c,
            s: r.s,
        })
    }
}

/// Parse a whole text, optionally cut at `max_degree`, stopping at the first
/// line after which the requested triangle is complete.
pub(super) fn parse(text: &str, max_degree: Option<usize>) -> Result<ParsedIcgem, IcgemParseError> {
    let mut parser = Parser::new(max_degree);
    for line in text.lines() {
        parser.feed_line(line)?;
        if parser.is_complete() {
            break;
        }
    }
    parser.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete degree-2 model with Fortran exponents and the header layout
    /// ICGEM actually emits.
    const MINIMAL: &str = "\
begin_of_head
product_type            gravity_field
modelname               TEST2
earth_gravity_constant  0.3986004415D+15
radius                  0.6378136300D+07
max_degree              2
errors                  no
norm                    fully_normalized
tide_system             zero_tide

key   L    M    C                    S
end_of_head
gfc   0    0    1.0D+00              0.0D+00
gfc   1    0    0.0                  0.0
gfc   1    1    0.0                  0.0
gfc   2    0   -0.484165143790815D-03 0.0
gfc   2    1   -0.206615509074176D-09 0.138441389137979D-08
gfc   2    2    0.243938357328313D-05 -0.140027370385934D-05
";

    fn whole(text: &str) -> Result<ParsedIcgem, IcgemParseError> {
        parse(text, None)
    }

    /// Insert `record` before the last coefficient, so the parser has not
    /// completed the triangle (and stopped reading) when it gets there.
    fn inject(record: &str) -> String {
        MINIMAL.replace("gfc   2    2", &alloc::format!("{record}\ngfc   2    2"))
    }

    #[test]
    fn parses_header_and_converts_to_km() {
        let p = whole(MINIMAL).unwrap();
        assert_eq!(p.gm_km3_s2, 398600.4415);
        assert_eq!(p.radius_km, 6378.1363);
        assert_eq!(p.max_degree, 2);
        assert_eq!(p.tide_system, TideSystem::ZeroTide);
        assert_eq!(p.model_name.as_deref(), Some("TEST2"));
        assert_eq!(p.c[tri_index(0, 0)], 1.0);
        assert_eq!(p.c[tri_index(2, 0)], -0.484165143790815e-3);
        assert_eq!(p.s[tri_index(2, 1)], 0.138441389137979e-8);
        assert_eq!(p.c[tri_index(2, 2)], 0.243938357328313e-5);
        assert_eq!(p.s[tri_index(2, 2)], -0.140027370385934e-5);
    }

    /// EGM2008's own header/record lines (as distributed by ICGEM) parse and
    /// keep C̄20 bit-exact: a golden value independent of any Rust code.
    #[test]
    fn egm2008_golden_lines() {
        let text = "\
product_type             gravity_field
modelname                EGM2008
earth_gravity_constant   3.986004415E+14
radius                   6378136.3
max_degree               2
errors                   formal
norm                     fully_normalized
tide_system              tide_free
end_of_head
gfc    0    0  1.000000000000E+00  0.000000000000E+00 0.0000E+00 0.0000E+00
gfc    2    0 -4.841651437908150E-04  0.000000000000E+00 7.4815E-11 0.0000E+00
gfc    2    1 -2.066155090741760E-10  1.384413891379790E-09 7.0630E-11 7.1667E-11
gfc    2    2  2.439383573283130E-06 -1.400273703859340E-06 7.2306E-11 7.3020E-11
";
        let p = whole(text).unwrap();
        assert_eq!(p.c[tri_index(2, 0)], -4.84165143790815e-4);
        assert_eq!(p.tide_system, TideSystem::TideFree);
        // Degree 1 absent → zero.
        assert_eq!(p.c[tri_index(1, 1)], 0.0);
    }

    /// The column-count message names the `errors` declaration the expected
    /// count came from, so a mismatched file can be fixed at the right line.
    #[test]
    fn error_column_count_must_match_header_and_the_message_names_the_header() {
        let text = MINIMAL.replace(
            "errors                  no",
            "errors                  calibrated",
        );
        match whole(&text) {
            Err(IcgemParseError::MalformedRecord { line: 13, reason }) => {
                assert!(reason.contains("`errors calibrated`"), "{reason}");
                assert!(reason.contains("expected 6 columns"), "{reason}");
                assert!(reason.contains("found 4"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        // Absent `errors` means no σ columns, and the message says so.
        let text = MINIMAL.replace("errors                  no\n", "").replace(
            "gfc   2    2    0.243938357328313D-05 -0.140027370385934D-05",
            "gfc 2 2 1.0 1.0 0.0 0.0",
        );
        match whole(&text) {
            Err(IcgemParseError::MalformedRecord { reason, .. }) => {
                assert!(reason.contains("`errors no (absent)`"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_time_variable_records() {
        for kind in ["gfct", "trnd", "dot", "asin", "acos"] {
            let text = inject(&alloc::format!("{kind} 2 0 1.0 0.0 20050101"));
            assert_eq!(
                whole(&text),
                Err(IcgemParseError::TimeVariableUnsupported {
                    line: 18,
                    kind: kind.to_string()
                })
            );
        }
    }

    #[test]
    fn rejects_unknown_record_kind() {
        assert!(matches!(
            whole(&inject("xyz 2 0 1.0 0.0")),
            Err(IcgemParseError::UnknownRecord { line: 18, .. })
        ));
    }

    #[test]
    fn rejects_unnormalized_and_non_gravity_products() {
        let text = MINIMAL.replace("fully_normalized", "unnormalized");
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::UnsupportedNormalization(
                "unnormalized".into()
            ))
        );
        let text = MINIMAL.replace("gravity_field", "topography");
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::NotAGravityField("topography".into()))
        );
    }

    /// `product_type` is checked when present and tolerated when absent
    /// (older files omit it; see module docs).
    #[test]
    fn product_type_is_optional() {
        let text = MINIMAL.replace("product_type            gravity_field\n", "");
        assert!(whole(&text).is_ok());
    }

    #[test]
    fn rejects_missing_end_of_head_and_missing_headers() {
        assert_eq!(
            whole("product_type gravity_field\n"),
            Err(IcgemParseError::MissingEndOfHead)
        );
        let text = MINIMAL.replace("earth_gravity_constant  0.3986004415D+15\n", "");
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::MissingHeader("earth_gravity_constant"))
        );
        let text = MINIMAL.replace("max_degree              2\n", "");
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::MissingHeader("max_degree"))
        );
        // The normalization has no safe default (see `resolve`).
        let text = MINIMAL.replace("norm                    fully_normalized\n", "");
        assert_eq!(whole(&text), Err(IcgemParseError::MissingHeader("norm")));
    }

    /// The parser sets no defaults for the keys it uses, so it cannot pick
    /// one of two declarations either.
    #[test]
    fn rejects_duplicate_header_keys() {
        for (key, line) in [
            ("norm", "norm unnormalized"),
            ("norm", "norm fully_normalized"),
            ("max_degree", "max_degree 5"),
            ("radius", "radius 1.0"),
            ("earth_gravity_constant", "earth_gravity_constant 1.0"),
            ("earth_gravity_constant", "gravity_constant 1.0"),
            ("errors", "errors formal"),
            ("product_type", "product_type gravity_field"),
            ("tide_system", "tide_system tide_free"),
            ("modelname", "modelname OTHER"),
        ] {
            let text = MINIMAL.replace("end_of_head", &alloc::format!("{line}\nend_of_head"));
            assert_eq!(
                whole(&text),
                Err(IcgemParseError::DuplicateHeader(key)),
                "{line}"
            );
        }
    }

    /// `tide_system` is recorded, never interpreted, so a spelling this parser
    /// does not know is `Unknown` — like an absent line — rather than a load
    /// failure.
    #[test]
    fn unrecognised_tide_system_spelling_reads_as_unknown() {
        let text = MINIMAL.replace(
            "tide_system             zero_tide",
            "tide_system             zero-tide",
        );
        assert_eq!(whole(&text).unwrap().tide_system, TideSystem::Unknown);
        let text = MINIMAL.replace("tide_system             zero_tide\n", "");
        assert_eq!(whole(&text).unwrap().tide_system, TideSystem::Unknown);
        let text = MINIMAL.replace("tide_system             zero_tide", "tide_system");
        assert_eq!(whole(&text).unwrap().tide_system, TideSystem::Unknown);
    }

    /// An unknown header keyword is provenance, not data: ignored.
    #[test]
    fn unknown_header_keys_are_ignored() {
        let text = MINIMAL.replace(
            "end_of_head",
            "generating_institute GFZ\nreference_date 20250101\nend_of_head",
        );
        assert!(whole(&text).is_ok());
    }

    /// Bounds are checked against the triangular layout before indexing,
    /// so a corrupt record cannot read or write out of range.
    #[test]
    fn rejects_bad_indices_duplicates_and_non_finite() {
        assert!(matches!(
            whole(&inject("gfc 3 0 1.0 0.0")),
            Err(IcgemParseError::IndexOutOfRange {
                degree: 3,
                order: 0,
                ..
            })
        ));
        assert!(matches!(
            whole(&inject("gfc 2 3 1.0 0.0")),
            Err(IcgemParseError::IndexOutOfRange {
                degree: 2,
                order: 3,
                ..
            })
        ));
        assert!(matches!(
            whole(&inject("gfc 2 1 1.0 0.0")),
            Err(IcgemParseError::DuplicateCoefficient {
                degree: 2,
                order: 1,
                ..
            })
        ));
        assert!(matches!(
            whole(&inject("gfc x 1 1.0 0.0")),
            Err(IcgemParseError::MalformedRecord { .. })
        ));
        let text = MINIMAL.replace("0.243938357328313D-05", "NaN");
        assert!(matches!(
            whole(&text),
            Err(IcgemParseError::NonFiniteCoefficient {
                degree: 2,
                order: 2,
                ..
            })
        ));
        let text = MINIMAL.replace("0.243938357328313D-05", "inf");
        assert!(matches!(
            whole(&text),
            Err(IcgemParseError::NonFiniteCoefficient { .. })
        ));
    }

    #[test]
    fn rejects_non_unit_c00_and_non_zero_degree_one() {
        let text = MINIMAL.replace("gfc   0    0    1.0D+00", "gfc   0    0    0.5D+00");
        assert!(matches!(whole(&text), Err(IcgemParseError::UnexpectedC00(v)) if v == 0.5));
        let text = MINIMAL.replace(
            "gfc   1    1    0.0                  0.0",
            "gfc 1 1 0.0 1e-9",
        );
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::NonZeroDegreeOne {
                degree: 1,
                order: 1
            })
        );
    }

    #[test]
    fn requires_every_coefficient_from_degree_two_up() {
        let text = MINIMAL.replace(
            "gfc   2    1   -0.206615509074176D-09 0.138441389137979D-08\n",
            "",
        );
        assert_eq!(
            whole(&text),
            Err(IcgemParseError::MissingCoefficient {
                degree: 2,
                order: 1
            })
        );
    }

    #[test]
    fn degree_zero_and_one_may_be_omitted() {
        let text = MINIMAL
            .replace("gfc   0    0    1.0D+00              0.0D+00\n", "")
            .replace("gfc   1    0    0.0                  0.0\n", "")
            .replace("gfc   1    1    0.0                  0.0\n", "");
        let p = whole(&text).unwrap();
        assert_eq!(p.c[tri_index(0, 0)], 1.0);
        assert_eq!(p.c[tri_index(1, 0)], 0.0);
    }

    /// `max_degree` beyond `MAX_DEGREE` is refused at the header, before
    /// overflowing the triangular size or allocating gigabytes.
    #[test]
    fn rejects_max_degree_above_the_supported_limit() {
        for bad in ["2191", "100000", "18446744073709551615"] {
            let text = MINIMAL.replace(
                "max_degree              2",
                &alloc::format!("max_degree {bad}"),
            );
            match whole(&text) {
                Err(IcgemParseError::InvalidHeader("max_degree", _)) => {}
                other => panic!("{bad}: expected InvalidHeader(max_degree), got {other:?}"),
            }
        }
        // usize overflow in the integer parse itself is also an InvalidHeader.
        let text = MINIMAL.replace(
            "max_degree              2",
            "max_degree 99999999999999999999999",
        );
        assert!(matches!(
            whole(&text),
            Err(IcgemParseError::InvalidHeader("max_degree", _))
        ));
    }

    #[test]
    fn model_name_keeps_the_whole_line_after_the_keyword() {
        let text = MINIMAL.replace(
            "modelname               TEST2",
            "modelname               EIGEN-6C4 (static part)   ",
        );
        let p = whole(&text).unwrap();
        assert_eq!(p.model_name.as_deref(), Some("EIGEN-6C4 (static part)"));
        // A bare keyword with no name is `None`, not `Some("")`.
        let text = MINIMAL.replace("modelname               TEST2", "modelname");
        assert_eq!(whole(&text).unwrap().model_name, None);
    }

    #[test]
    fn fortran_exponent_parsing() {
        assert_eq!(parse_f64("0.5D+01"), Some(5.0));
        assert_eq!(parse_f64("-1.25d-2"), Some(-0.0125));
        assert_eq!(parse_f64("1e3"), Some(1000.0));
        assert_eq!(parse_f64("abc"), None);
    }

    /// A degree-4 text whose degree-4 record is deliberately malformed (an
    /// index that is not an integer fails before the cap skip), to show that
    /// a cap really stops the parser before it gets there.
    const DEGREE4_BROKEN_TAIL: &str = "\
earth_gravity_constant 3.986004415E+14
radius 6378136.3
max_degree 4
norm fully_normalized
end_of_head
gfc 0 0 1.0 0.0
gfc 1 0 0.0 0.0
gfc 1 1 0.0 0.0
gfc 2 0 -4.8e-4 0.0
gfc 2 1 0.0 0.0
gfc 2 2 2.4e-6 -1.4e-6
gfc 3 0 9.6e-7 0.0
gfc 3 1 0.0 0.0
gfc 3 2 0.0 0.0
gfc 3 3 0.0 0.0
gfc 4 x 1.0 0.0
";

    /// With a cap the parser stops once the requested triangle is complete:
    /// the broken degree-4 record is never read, and the arrays are sized
    /// for the cap rather than the file.
    #[test]
    fn a_degree_cap_stops_early_and_sizes_for_the_cap() {
        let p = parse(DEGREE4_BROKEN_TAIL, Some(3)).unwrap();
        assert_eq!(p.max_degree, 3);
        assert_eq!(p.c.len(), tri_len(3));
        assert_eq!(p.c[tri_index(3, 0)], 9.6e-7);
        // Without the cap the same text is rejected at the broken record.
        assert!(matches!(
            parse(DEGREE4_BROKEN_TAIL, None),
            Err(IcgemParseError::MalformedRecord { line: 16, .. })
        ));
    }

    /// Completion waits for the degree-0/1 records too, so their value checks
    /// still run when they come after the required triangle (unsorted file)
    /// or when the cap is below 2; a file that omits them is read to the end.
    #[test]
    fn early_stop_never_skips_the_degree_zero_and_one_checks() {
        // C00 record moved after the triangle, with a bad value.
        let text = MINIMAL
            .replace("gfc   0    0    1.0D+00              0.0D+00\n", "")
            .replace(
                "gfc   2    2    0.243938357328313D-05 -0.140027370385934D-05\n",
                "gfc   2    2    0.243938357328313D-05 -0.140027370385934D-05\ngfc 0 0 0.5 0.0\n",
            );
        assert!(matches!(
            parse(&text, Some(2)),
            Err(IcgemParseError::UnexpectedC00(v)) if v == 0.5
        ));
        // Cap below 2 still validates the degree-1 record.
        let text = MINIMAL.replace(
            "gfc   1    1    0.0                  0.0",
            "gfc 1 1 0.0 1e-9",
        );
        assert_eq!(
            parse(&text, Some(1)),
            Err(IcgemParseError::NonZeroDegreeOne {
                degree: 1,
                order: 1
            })
        );
        // Degree 0/1 omitted: never "complete", but the whole file parses.
        let text = DEGREE4_BROKEN_TAIL
            .replace("gfc 0 0 1.0 0.0\n", "")
            .replace("gfc 1 0 0.0 0.0\n", "")
            .replace("gfc 1 1 0.0 0.0\n", "")
            .replace("gfc 4 x 1.0 0.0\n", "");
        let mut parser = Parser::new(Some(3));
        for line in text.lines() {
            parser.feed_line(line).unwrap();
        }
        assert!(!parser.is_complete());
        assert_eq!(parser.finish().unwrap().max_degree, 3);
    }

    /// Records above the cap that arrive *before* the triangle is complete
    /// (an unsorted file) are index-checked and skipped, not stored.
    #[test]
    fn records_above_the_cap_are_skipped_when_interleaved() {
        let text = DEGREE4_BROKEN_TAIL
            .replace("gfc 4 x 1.0 0.0\n", "")
            .replace("gfc 2 1 0.0 0.0\n", "gfc 4 4 1.0 1.0\ngfc 2 1 0.0 0.0\n");
        let p = parse(&text, Some(3)).unwrap();
        assert_eq!(p.max_degree, 3);
        assert_eq!(p.c.len(), tri_len(3));
        // … but a record outside the *file's* triangle is still an error.
        let text = text.replace("gfc 4 4 1.0 1.0\n", "gfc 5 0 1.0 1.0\n");
        assert!(matches!(
            parse(&text, Some(3)),
            Err(IcgemParseError::IndexOutOfRange { degree: 5, .. })
        ));
    }

    /// Asking for more than the file has is an error, not a silent clamp.
    #[test]
    fn a_cap_above_the_files_degree_is_refused() {
        assert_eq!(
            parse(DEGREE4_BROKEN_TAIL, Some(5)),
            Err(IcgemParseError::DegreeUnavailable {
                requested: 5,
                available: 4
            })
        );
    }

    /// A cap equal to the file's degree reads everything, like no cap.
    #[test]
    fn a_cap_equal_to_the_files_degree_reads_the_whole_file() {
        let capped = parse(MINIMAL, Some(2)).unwrap();
        let whole = whole(MINIMAL).unwrap();
        assert_eq!(capped.max_degree, whole.max_degree);
        assert_eq!(capped.c, whole.c);
        assert_eq!(capped.s, whole.s);
    }

    #[test]
    fn is_complete_flips_exactly_at_the_last_required_record() {
        let mut parser = Parser::new(Some(2));
        let lines: Vec<&str> = MINIMAL.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            parser.feed_line(line).unwrap();
            assert_eq!(parser.is_complete(), i == lines.len() - 1, "line {}", i + 1);
        }
        // Whole-file mode has the same completion point here (cap = 2).
        let mut parser = Parser::new(None);
        for line in &lines[..lines.len() - 1] {
            parser.feed_line(line).unwrap();
        }
        assert!(!parser.is_complete());
        parser.feed_line(lines[lines.len() - 1]).unwrap();
        assert!(parser.is_complete());
    }
}
