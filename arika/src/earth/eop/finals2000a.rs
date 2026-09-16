//! Parser for IERS finals2000A fixed-column format.
//!
//! Reference: USNO finals2000A format specification.
//! <https://maia.usno.navy.mil/ser7/readme.finals2000A>
//!
//! Column layout (1-indexed):
//!   1-2   : Year (YY)
//!   3-4   : Month
//!   5-6   : Day
//!   8-15  : MJD (F8.2)
//!   16    : I/P flag (polar motion)
//!   18-27 : x pole [arcsec], Bulletin A
//!   37-46 : y pole [arcsec], Bulletin A
//!   57    : I/P flag (UT1-UTC)
//!   58-68 : UT1-UTC [seconds], Bulletin A
//!   79-86 : LOD [milliseconds], Bulletin A
//!   97    : I/P flag (nutation)
//!   98-106: dX [mas], Bulletin A
//!   116-124: dY [mas], Bulletin A
//!
//! Bulletin B values (final quality, preferred when present):
//!   135-144: x pole [arcsec]
//!   145-154: y pole [arcsec]
//!   155-165: UT1-UTC [seconds]
//!   166-175: dX [mas]
//!   176-185: dY [mas]

use alloc::string::ToString;
use alloc::vec::Vec;

use super::entry::EopEntry;
use super::error::EopParseError;

/// Parser for IERS `finals2000A.all` / `finals2000A.data` / `finals2000A.daily`.
pub struct Finals2000A;

/// The Bulletin A columns a row must carry to be an entry (0-indexed byte
/// ranges): x pole, y pole, UT1-UTC. A row with all three blank is the file's
/// padded tail; a row with only some blank is malformed.
const REQUIRED_A_COLUMNS: [(usize, usize); 3] = [(17, 27), (36, 46), (58, 68)];

impl Finals2000A {
    /// Parse a finals2000A text file into a vector of EOP entries.
    ///
    /// Bulletin B values are preferred when available; otherwise Bulletin A
    /// values are used (matching Orekit's behavior).
    ///
    /// Lines shorter than 68 characters are silently skipped (header lines,
    /// blank lines), and so are rows that carry a date and MJD but no
    /// Bulletin A values at all: the published `finals2000A.all` ends with
    /// about fifty such rows, padded to full width, for the dates past the
    /// last prediction. A row with *some* of the required values is still an
    /// error — that is a malformed row, not the end of the series. Only rows
    /// with a valid MJD and Bulletin A pole + UT1-UTC values are included.
    pub fn parse(text: &str) -> Result<Vec<EopEntry>, EopParseError> {
        let mut entries = Vec::new();
        let mut prev_mjd: Option<f64> = None;

        for (line_idx, line) in text.lines().enumerate() {
            let line_num = line_idx + 1;

            // Skip short lines (headers, blanks)
            if line.len() < 68 {
                continue;
            }

            // Parse MJD (cols 8-15, 0-indexed: 7..15)
            let mjd_str = &line[7..15];
            let mjd: f64 = match mjd_str.trim().parse() {
                Ok(v) => v,
                Err(_) => continue, // skip non-data lines
            };

            // A NaN parses as a number and then compares as neither greater
            // nor smaller, so the monotonicity test below cannot see it. Named
            // here, where the line number is still in hand.
            if !mjd.is_finite() {
                return Err(EopParseError::InvalidNumber {
                    line: line_num,
                    column: "MJD",
                    value: mjd_str.trim().to_string(),
                });
            }

            // The blank tail of the published file: dated rows with every
            // Bulletin A column empty. Not data, so not an error either.
            if REQUIRED_A_COLUMNS
                .iter()
                .all(|&(start, end)| line[start..end].trim().is_empty())
            {
                continue;
            }

            // Check monotonicity
            if let Some(prev) = prev_mjd.filter(|&p| mjd <= p) {
                return Err(EopParseError::NonMonotonicMjd {
                    line: line_num,
                    previous: prev,
                    current: mjd,
                });
            }

            // Parse Bulletin A pole and UT1-UTC (required; see
            // `REQUIRED_A_COLUMNS`)
            let [(xp_s, xp_e), (yp_s, yp_e), (dut1_s, dut1_e)] = REQUIRED_A_COLUMNS;
            let xp_a = parse_col(line, xp_s, xp_e, "xp_A", line_num)?;
            let yp_a = parse_col(line, yp_s, yp_e, "yp_A", line_num)?;
            let dut1_a = parse_col(line, dut1_s, dut1_e, "dut1_A", line_num)?;

            // Parse Bulletin A LOD [ms] (optional)
            let lod_a = parse_col_opt(line, 78, 86, "lod_A", line_num)?;

            // Parse Bulletin A nutation (optional; `parse_col_opt` reports a
            // line that stops inside the column as having no value)
            let dx_a = parse_col_opt(line, 97, 106, "dX_A", line_num)?;
            let dy_a = parse_col_opt(line, 116, 125, "dY_A", line_num)?;

            // Parse Bulletin B values (preferred when present)
            let xp_b = parse_col_opt(line, 134, 144, "xp_B", line_num)?;
            let yp_b = parse_col_opt(line, 144, 154, "yp_B", line_num)?;
            let dut1_b = parse_col_opt(line, 154, 165, "dut1_B", line_num)?;
            let dx_b = parse_col_opt(line, 165, 175, "dX_B", line_num)?;
            let dy_b = parse_col_opt(line, 175, 185, "dY_B", line_num)?;

            // Prefer Bulletin B when available
            let xp = xp_b.unwrap_or(xp_a);
            let yp = yp_b.unwrap_or(yp_a);
            let dut1 = dut1_b.unwrap_or(dut1_a);
            let dx = dx_b.or(dx_a);
            let dy = dy_b.or(dy_a);
            // LOD: only from Bulletin A (B doesn't have it)
            let lod = lod_a.map(|ms| ms * 1e-3); // ms -> seconds

            entries.push(EopEntry {
                mjd,
                xp,
                yp,
                dut1,
                lod,
                dx,
                dy,
            });

            prev_mjd = Some(mjd);
        }

        if entries.is_empty() {
            return Err(EopParseError::Empty);
        }

        Ok(entries)
    }
}

/// Parse a required fixed-column field.
fn parse_col(
    line: &str,
    start: usize,
    end: usize,
    column: &'static str,
    line_num: usize,
) -> Result<f64, EopParseError> {
    let end = end.min(line.len());
    let s = line[start..end].trim();
    let value = s.parse::<f64>().map_err(|_| EopParseError::InvalidNumber {
        line: line_num,
        column,
        value: s.to_string(),
    })?;
    finite(value, column, line_num, s)
}

/// Refuse a parsed value that is not finite.
///
/// `f64::from_str` accepts `NaN`, `inf` and `-inf`, and IERS publishes none of
/// them: a column spelling one of those is damage, and letting it through put
/// the value straight into a lookup answer. A Bulletin B `NaN` also wins over
/// a good Bulletin A reading, since the B column is preferred when present.
fn finite(
    value: f64,
    column: &'static str,
    line_num: usize,
    text: &str,
) -> Result<f64, EopParseError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(EopParseError::InvalidNumber {
            line: line_num,
            column,
            value: text.to_string(),
        })
    }
}

/// Parse an optional fixed-column field: `None` where the column is blank or
/// the line stops before it, an error where it holds something that is not a
/// number.
///
/// A blank column means IERS published no value there, which a caller can
/// answer with the model alone. A column holding `0.3O0` means the row is
/// corrupt, and reading it as "no value" turned that into the same
/// model-only answer — the required columns have always been an error, and
/// these now agree. `NaN`, `inf` and `-inf` are refused as well: they parse,
/// and IERS publishes none of them.
fn parse_col_opt(
    line: &str,
    start: usize,
    end: usize,
    column: &'static str,
    line_num: usize,
) -> Result<Option<f64>, EopParseError> {
    // The columns are fixed-width, so a line that stops inside one carries no
    // value there. Clamping the slice to the line instead read the prefix as a
    // number: a row cut off inside LOD's `79..86` parsed `line[78..82]` and
    // came back as a different, smaller reading.
    if line.len() < end {
        return Ok(None);
    }
    let s = line[start..end].trim();
    if s.is_empty() {
        return Ok(None);
    }
    let value = s.parse::<f64>().map_err(|_| EopParseError::InvalidNumber {
        line: line_num,
        column,
        value: s.to_string(),
    })?;
    finite(value, column, line_num, s).map(Some)
}
