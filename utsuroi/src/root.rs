//! Boundaries a state decides, located inside the step that crosses them.
//!
//! A burn window's edges are known times, so a [`Segments`](crate::Segments)
//! walk can cut the span at them and every step lands on a boundary exactly. A
//! reaction wheel saturating or a tank running dry is not known in advance: the
//! time follows from the state, and the only way to land on it is to look for
//! the crossing while stepping. That is what a [`RootEvent`] describes, and its
//! contract differs from the `event_check` the plain `advance_to` takes:
//!
//! - the boundary is the zero of a signed, continuous function, so a crossing
//!   can be bracketed rather than merely noticed one step late
//! - the event says which direction counts, and whether reaching the boundary
//!   ends the walk
//! - detection reads the raw candidate, before
//!   [`OdeState::project`](crate::OdeState::project) — a projection pulls the
//!   state back onto its constraint surface, and can erase the sign change that
//!   shows the crossing
//!
//! # What a walk with root events guarantees
//!
//! The state and time a stepper holds afterwards are the ones at the boundary,
//! projected, and the callback is called there. States tried during the search
//! reach neither the callback nor the projection. [`RootSet::hits`] lists every
//! event that crossed within the final bracket, so a caller that has to break a
//! tie between two wheels saturating together sees both.
//!
//! # What the caller owes
//!
//! **The right-hand side must not change while a root is being searched for.**
//! Localization re-steps from the last committed state with shorter and shorter
//! widths; if the system switches its discrete mode at the boundary, a method's
//! stages then mix the two modes and the bisection converges on the wrong time.
//! `y' = 1` below `y = 1` and `y' = 0` at or above it, with `g = y - 1`,
//! reaches the boundary at `t = 1`; re-stepping RK4 across the switch converges
//! on a width of `6r/5` for a remaining distance `r`, which reports the arrival
//! `0.2 r` late. Keep the pre-root mode frozen for the whole search and apply
//! the change after landing; a continuous state feedback is fine to evaluate at
//! every stage, it is the discrete mode that has to hold still.
//!
//! **One step may hold at most one change of sign of each event's value**, in
//! either direction — not one crossing in the direction the event counts. Two
//! events may each change sign in the same step; that is what a group of
//! simultaneous roots is. What
//! detection reads is the value at the step's start against the value at a
//! trial end, so a step holding two changes of sign reports nothing at all, and
//! one holding three converges on the last: for
//! `g = (t - 0.2)(t - 0.4)(t - 0.8)` over `[0, 1]` the first trial at `0.5` has
//! the sign the step started with, which discards `0.2` and `0.4` and lands on
//! `0.8`. A step where the value dips across zero and comes back also reports
//! nothing, even though only one of those two changes is in the counted
//! direction. Bound the step size so a step holds one; the search can only read
//! the value at times it picks, so it cannot check this for the caller.
//!
//! Detection needs a sign change, so a walk whose very first state sits exactly
//! on a boundary reports no root for it: that is also the state a previous root
//! leaves behind, and the two cannot be told apart from the value alone.
//!
//! # Storage
//!
//! utsuroi does not allocate, so [`RootSet`] carries its own fixed-size
//! storage: the event count is a const parameter, and the guards and hits live
//! in the set. The caller holds the set across resumptions, which is what stops
//! a non-terminal root from being found again while the state sits on its
//! boundary.

use crate::IntegrationError;

/// Which way across zero counts as reaching the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crossing {
    /// From negative to positive.
    Rising,
    /// From positive to negative.
    Falling,
    /// Either direction.
    Either,
}

impl Crossing {
    /// Whether a move from `before` to `after` is the crossing this counts.
    ///
    /// Landing exactly on zero counts, from whichever side; leaving zero does
    /// not. A non-terminal root leaves the state on the boundary, and a walk
    /// resumed from there must not report the departure as a fresh crossing.
    fn matches(self, before: f64, after: f64) -> bool {
        let rising = before < 0.0 && after >= 0.0;
        let falling = before > 0.0 && after <= 0.0;
        match self {
            Crossing::Rising => rising,
            Crossing::Falling => falling,
            Crossing::Either => rising || falling,
        }
    }
}

/// A boundary the state decides, as the zero of a signed function.
pub trait RootEvent<Y> {
    /// The signed value at `(t, y)`. Continuous in both, and finite wherever
    /// the walk can reach: a non-finite value stops the walk with
    /// [`IntegrationError::NonFiniteRootValue`] rather than being guessed
    /// about, since its sign says nothing about where a crossing is.
    ///
    /// One step may hold at most one change of sign of this value, whichever
    /// direction each change is in — see the module documentation for what the
    /// search reports when a step holds more, and why it cannot detect the case
    /// itself.
    fn value(&self, t: f64, y: &Y) -> f64;

    /// Which direction across zero counts. [`Crossing::Either`], unless
    /// overridden.
    fn crossing(&self) -> Crossing {
        Crossing::Either
    }

    /// Whether reaching this boundary ends the walk. Terminal, unless
    /// overridden: a non-terminal root hands control back at the boundary and
    /// the caller resumes from there.
    fn terminal(&self) -> bool {
        true
    }

    /// Order among events that cross within the same bracket. Lower is
    /// reported first, and events of equal priority keep the order they were
    /// registered in.
    fn priority(&self) -> i32 {
        0
    }

    /// Width of `|value|` within which the state still counts as being on this
    /// boundary, in the value's own units.
    ///
    /// Zero, unless overridden, and it has to be finite and not negative:
    /// [`RootSet::new`] refuses the rest, since a negative width re-arms the
    /// guard at once and a non-finite one never re-arms it.
    ///
    /// What this settles is a constraint the state moves *along* — a wheel held
    /// at its saturation torque — where `value` stays at a jitter around zero
    /// for many steps rather than at zero, and each change of sign in that
    /// jitter would otherwise be a fresh crossing. The step that leaves a root
    /// behind is a separate matter, suppressed exactly, by time; see
    /// [`RootGuard`]. The width cannot be derived from
    /// [`RootSearch::t_tolerance`], which is a time.
    fn boundary_tolerance(&self) -> f64 {
        0.0
    }
}

/// What one event knows between steps, and across resumptions.
///
/// Two things keep a root from being found again, because the state a root
/// leaves behind is not exactly on the boundary — the search stops on the far
/// side of a bracket, so the value there is a small non-zero rather than zero:
///
/// - the time the root was reported, together with the value the root left
///   behind. A step that starts at that time with the value still on that side
///   is the one leaving the root, and it does not report the same event again.
///   Both comparisons are exact and need no tolerance. A caller that moves the
///   value to the other side of zero, and clear of the event's
///   [`boundary_tolerance`](RootEvent::boundary_tolerance), has moved the state
///   off that root, so the step it then takes reports a crossing as any other
///   would.
/// - whether the value is still within the event's
///   [`boundary_tolerance`](RootEvent::boundary_tolerance), for a state that
///   goes on moving along the boundary over many steps.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RootGuard {
    /// Time of the root this event last reported, while the walk is still
    /// standing on it.
    at: Option<f64>,
    /// The value at that root, whose side is what tells a departure from a
    /// fresh arrival.
    left: f64,
    /// Set while the committed state is one a root of this event left behind,
    /// and cleared once the value leaves the event's boundary tolerance.
    on_boundary: bool,
}

/// Whether two values are on the same side of zero.
///
/// Zero is on no side, so it matches only zero: a root that landed exactly on
/// the boundary left the value with no side, and any non-zero value the caller
/// puts there has moved it off.
fn same_side(a: f64, b: f64) -> bool {
    if a == 0.0 || b == 0.0 {
        a == b
    } else {
        (a > 0.0) == (b > 0.0)
    }
}

impl RootGuard {
    /// A guard for an event no root has fired on yet.
    pub const fn new() -> Self {
        Self {
            at: None,
            left: 0.0,
            on_boundary: false,
        }
    }

    /// The time of the root this event last reported, if the walk is still
    /// standing on it.
    pub fn at(&self) -> Option<f64> {
        self.at
    }

    /// Whether this event is suppressed because the state is on its boundary.
    pub fn is_on_boundary(&self) -> bool {
        self.on_boundary
    }
}

/// One event's crossing, located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootHit {
    /// Index of the event in the set, which is the order it was registered in.
    pub event: usize,
    /// Whether this event asked to end the walk.
    pub terminal: bool,
    /// The event's priority, as it reported it.
    pub priority: i32,
}

/// How a walk with root events ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RootOutcome {
    /// The target time was reached with no root in the way.
    Reached,
    /// One or more events crossed. [`RootSet::hits`] lists them, ordered by
    /// priority and then by registration order.
    Roots {
        /// The time the walk stopped at, which is where the state now is. Every
        /// hit shares it.
        t: f64,
        /// Width of the bracket the search ended with [s]. The time is
        /// uncertain by about this much numerically; how far it is from the
        /// true crossing also depends on the state error and on how flat the
        /// value is there.
        bracket: f64,
        /// Whether any of the hits asked to end the walk. A caller that resumes
        /// anyway takes its next step from the boundary.
        terminal: bool,
    },
}

/// How hard to look for a crossing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootSearch {
    /// Bracket width to stop at [s]. The search ends once the interval holding
    /// the crossing is this narrow, or once halving it no longer changes the
    /// bracket in f64.
    pub t_tolerance: f64,
    /// Cap on bisection iterations. A value that behaves unlike a continuous
    /// function cannot make the search spin: the walk fails with
    /// [`IntegrationError::RootNotLocalized`] instead.
    pub max_iterations: u32,
}

impl Default for RootSearch {
    fn default() -> Self {
        Self {
            // A millisecond is finer than any cadence orts samples at, and 60
            // halvings take a day-wide step well below it.
            t_tolerance: 1e-3,
            max_iterations: 60,
        }
    }
}

impl RootSearch {
    fn validate(&self) -> Result<(), IntegrationError> {
        if self.t_tolerance.is_finite() && self.t_tolerance > 0.0 && self.max_iterations > 0 {
            Ok(())
        } else {
            Err(IntegrationError::InvalidRootSearch {
                t_tolerance: self.t_tolerance,
                max_iterations: self.max_iterations,
            })
        }
    }
}

/// The root events of a walk, with the state each one carries between steps.
///
/// Built once and handed to `advance_to_roots` for every target, so the guards
/// survive a resumption: a non-terminal root leaves the state on its boundary,
/// and the guard is what keeps the next step from reporting the departure as a
/// new crossing.
pub struct RootSet<'a, Y, const N: usize> {
    events: [&'a dyn RootEvent<Y>; N],
    guards: [RootGuard; N],
    /// Each event's boundary tolerance, read once and checked. Held here rather
    /// than asked for per step, so the width a guard re-arms on cannot change
    /// under the walk.
    boundary: [f64; N],
    search: RootSearch,
    /// Values at the state the stepper has committed. Refreshed at the start of
    /// every walk, because the caller is free to change the state between them.
    values: [f64; N],
    /// Events that crossed over the step being examined.
    candidates: [bool; N],
    hits: [RootHit; N],
    hit_count: usize,
}

impl<'a, Y, const N: usize> RootSet<'a, Y, N> {
    /// Pair events with fresh guards.
    pub fn new(
        events: [&'a dyn RootEvent<Y>; N],
        search: RootSearch,
    ) -> Result<Self, IntegrationError> {
        search.validate()?;
        let mut boundary = [0.0; N];
        for (index, event) in events.iter().enumerate() {
            let tolerance = event.boundary_tolerance();
            if !(tolerance.is_finite() && tolerance >= 0.0) {
                return Err(IntegrationError::InvalidBoundaryTolerance {
                    event: index,
                    tolerance,
                });
            }
            boundary[index] = tolerance;
        }
        Ok(Self {
            events,
            guards: [RootGuard::new(); N],
            boundary,
            search,
            values: [0.0; N],
            candidates: [false; N],
            hits: [RootHit {
                event: 0,
                terminal: false,
                priority: 0,
            }; N],
            hit_count: 0,
        })
    }

    /// The events that crossed within the bracket the last walk ended on, in
    /// the order they should be handled: by priority, then by registration.
    ///
    /// Empty after a walk that reached its target.
    pub fn hits(&self) -> &[RootHit] {
        &self.hits[..self.hit_count]
    }

    /// One event's guard, for a caller checking whether the state is sitting on
    /// that boundary.
    pub fn guard(&self, event: usize) -> RootGuard {
        self.guards[event]
    }

    /// How many events are in the set.
    pub fn len(&self) -> usize {
        N
    }

    /// Whether the set has no events, in which case a walk over it is the plain
    /// one.
    pub fn is_empty(&self) -> bool {
        N == 0
    }

    /// Values at a state, written into `out`.
    fn values_at(&self, t: f64, y: &Y, out: &mut [f64; N]) -> Result<(), IntegrationError> {
        for (index, event) in self.events.iter().enumerate() {
            let value = event.value(t, y);
            if !value.is_finite() {
                return Err(IntegrationError::NonFiniteRootValue { t, event: index });
            }
            out[index] = value;
        }
        Ok(())
    }

    /// Read the values at the state a walk starts from, and re-arm the sliding
    /// guard of every event whose value has left its boundary.
    ///
    /// The values are read again rather than carried over from the last walk:
    /// after a non-terminal root the caller changes the state, so the ones from
    /// before that change describe a different trajectory. A guard whose root
    /// time is this walk's start keeps it — that is the resumption the guard
    /// exists for; one from an earlier time is dropped, since the walk has
    /// moved on and the accessor would otherwise name a root it has left.
    pub(crate) fn begin(&mut self, t: f64, y: &Y) -> Result<(), IntegrationError> {
        let mut values = [0.0; N];
        self.values_at(t, y, &mut values)?;
        self.values = values;
        self.hit_count = 0;
        for (index, &value) in values.iter().enumerate() {
            if self.guards[index].at != Some(t) {
                self.guards[index].at = None;
            }
            if self.off_boundary(index, value) {
                self.guards[index].on_boundary = false;
            }
        }
        Ok(())
    }

    /// Whether a value is far enough from zero for the event's guard to re-arm.
    fn off_boundary(&self, index: usize, value: f64) -> bool {
        value.abs() > self.boundary[index]
    }

    /// Whether the event at `index` counts a move from `before` to `after` over
    /// a step starting at `t_start`, with its guard taken into account.
    fn crossed(&self, index: usize, t_start: f64, before: f64, after: f64) -> bool {
        let guard = &self.guards[index];
        if guard.at == Some(t_start) && same_side(before, guard.left) {
            // This step starts on a root of this very event, with the value
            // still on the side that root left it. The search stops on the far
            // side of a bracket, so that value is a small non-zero rather than
            // zero, and the change of sign this step sees is the one already
            // reported.
            return false;
        }
        if self.guards[index].on_boundary && !self.off_boundary(index, before) {
            // The state has been moving along the boundary since a root of this
            // event, within the width the event calls "still on it".
            return false;
        }
        self.events[index].crossing().matches(before, after)
    }

    /// Whether any event crosses over a step landing on `values`, recording
    /// which ones so the search can ignore the rest.
    fn mark_candidates(&mut self, t_start: f64, values: &[f64; N]) -> bool {
        let mut any = false;
        for (index, &after) in values.iter().enumerate() {
            let crossed = self.crossed(index, t_start, self.values[index], after);
            self.candidates[index] = crossed;
            any |= crossed;
        }
        any
    }

    /// Whether any of the events already marked as candidates crosses over a
    /// shorter step landing on `values`.
    fn any_candidate_crosses(&self, t_start: f64, values: &[f64; N]) -> bool {
        (0..N).any(|index| {
            self.candidates[index]
                && self.crossed(index, t_start, self.values[index], values[index])
        })
    }

    /// Record the events that cross over the located step, ordered by priority
    /// and then by registration, and report whether any is terminal.
    fn record_hits(&mut self, t_start: f64, values: &[f64; N]) -> bool {
        self.hit_count = 0;
        for (index, &after) in values.iter().enumerate() {
            if self.candidates[index] && self.crossed(index, t_start, self.values[index], after) {
                let event = self.events[index];
                self.hits[self.hit_count] = RootHit {
                    event: index,
                    terminal: event.terminal(),
                    priority: event.priority(),
                };
                self.hit_count += 1;
            }
        }
        // Insertion sort on priority, which keeps registration order among
        // equals. The set is small and no allocation is available.
        for i in 1..self.hit_count {
            let mut j = i;
            while j > 0 && self.hits[j - 1].priority > self.hits[j].priority {
                self.hits.swap(j - 1, j);
                j -= 1;
            }
        }
        self.hits[..self.hit_count].iter().any(|hit| hit.terminal)
    }

    /// The values at a state the stepper is about to commit.
    ///
    /// Separate from [`apply`](Self::apply) so a non-finite value here is
    /// reported while the stepper still holds its previous state: a projection
    /// can produce one that the raw candidate did not have, and an error must
    /// not leave the walk standing on a state it also refused. A failure here
    /// also drops the hits the search had recorded, so [`hits`](Self::hits)
    /// describes a root the walk committed rather than one it gave up on.
    pub(crate) fn check(&mut self, t: f64, y: &Y) -> Result<[f64; N], IntegrationError> {
        let mut values = [0.0; N];
        if let Err(e) = self.values_at(t, y, &mut values) {
            self.hit_count = 0;
            return Err(e);
        }
        Ok(values)
    }

    /// Drop the hits the search recorded, for a stepper that refuses the state
    /// they belong to before it asks for their values.
    pub(crate) fn discard_hits(&mut self) {
        self.hit_count = 0;
    }

    /// Record where the walk now is: the events that just fired are on their
    /// boundary at `t`, and the rest have moved on.
    ///
    /// `values` are the ones [`check`](Self::check) returned for the same state,
    /// so this cannot fail and the stepper can update itself first.
    pub(crate) fn apply(&mut self, t: f64, values: &[f64; N]) {
        let mut fired = [false; N];
        for hit in &self.hits[..self.hit_count] {
            fired[hit.event] = true;
        }
        for (index, &value) in values.iter().enumerate() {
            if fired[index] {
                self.guards[index].at = Some(t);
                self.guards[index].left = value;
                self.guards[index].on_boundary = true;
            } else {
                // The walk has taken a step that did not end on this event's
                // boundary, so it is no longer standing on the root it reported.
                self.guards[index].at = None;
                if self.off_boundary(index, value) {
                    self.guards[index].on_boundary = false;
                }
            }
        }
        self.values = *values;
    }
}

/// What examining one committed step for roots found.
pub(crate) enum StepRoots<Y> {
    /// No event crossed; the step's own candidate is what to commit.
    None,
    /// A root was located. `state` is the raw state at `t`, which the stepper
    /// projects and commits in place of the step's candidate.
    Found {
        t: f64,
        state: Y,
        bracket: f64,
        terminal: bool,
    },
}

impl<Y: Clone, const N: usize> RootSet<'_, Y, N> {
    /// Examine one step, from the committed `(t0, y0)` to the raw candidate
    /// `y_end` of width `h`, and locate the earliest crossing in it.
    ///
    /// `raw_step(width)` must re-step from `(t0, y0)` by `width` with the same
    /// solver and the same right-hand side, and return the raw candidate:
    /// unprojected, and without touching any step-size control or cached stage
    /// derivative. Bisection is the only localizer available, since neither
    /// adaptive solver carries a dense output.
    pub(crate) fn scan_step<R>(
        &mut self,
        t0: f64,
        h: f64,
        y_end: &Y,
        mut raw_step: R,
    ) -> Result<StepRoots<Y>, IntegrationError>
    where
        R: FnMut(f64) -> Result<Y, IntegrationError>,
    {
        if N == 0 {
            return Ok(StepRoots::None);
        }
        let mut values_hi = [0.0; N];
        self.values_at(t0 + h, y_end, &mut values_hi)?;
        if !self.mark_candidates(t0, &values_hi) {
            return Ok(StepRoots::None);
        }

        // Invariant: a candidate crosses within `(t0, t0 + hi]`, and none
        // within `(t0, t0 + lo]`. Both ends re-step from `(t0, y0)`, so the
        // step-size control and the mode of the system stay where they were.
        let mut lo = 0.0_f64;
        let mut hi = h;
        let mut y_hi = y_end.clone();
        let mut iterations = 0_u32;
        while hi - lo > self.search.t_tolerance {
            if iterations >= self.search.max_iterations {
                return Err(IntegrationError::RootNotLocalized {
                    t: t0,
                    bracket: hi - lo,
                });
            }
            iterations += 1;
            let mid = lo + (hi - lo) / 2.0;
            if mid <= lo || mid >= hi {
                // f64 has no width left between the two ends: the bracket is as
                // tight as the clock can express, which is tighter than asked.
                break;
            }
            let y_mid = raw_step(mid)?;
            let mut values_mid = [0.0; N];
            self.values_at(t0 + mid, &y_mid, &mut values_mid)?;
            if self.any_candidate_crosses(t0, &values_mid) {
                hi = mid;
                y_hi = y_mid;
                values_hi = values_mid;
            } else {
                lo = mid;
            }
        }

        let terminal = self.record_hits(t0, &values_hi);
        Ok(StepRoots::Found {
            t: t0 + hi,
            state: y_hi,
            bracket: hi - lo,
            terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rising_event_ignores_a_fall_and_the_other_way_round() {
        assert!(Crossing::Rising.matches(-1.0, 1.0));
        assert!(!Crossing::Rising.matches(1.0, -1.0));
        assert!(Crossing::Falling.matches(1.0, -1.0));
        assert!(!Crossing::Falling.matches(-1.0, 1.0));
        assert!(Crossing::Either.matches(-1.0, 1.0));
        assert!(Crossing::Either.matches(1.0, -1.0));
    }

    #[test]
    fn landing_exactly_on_zero_is_a_crossing_but_leaving_it_is_not() {
        assert!(Crossing::Rising.matches(-1.0, 0.0));
        assert!(Crossing::Falling.matches(1.0, 0.0));
        assert!(!Crossing::Rising.matches(0.0, 1.0));
        assert!(!Crossing::Falling.matches(0.0, -1.0));
        assert!(!Crossing::Either.matches(0.0, 1.0));
        assert!(!Crossing::Either.matches(0.0, -1.0));
    }

    struct Level {
        level: f64,
        crossing: Crossing,
        terminal: bool,
        priority: i32,
        boundary: f64,
    }

    impl Level {
        fn at(level: f64) -> Self {
            Self {
                level,
                crossing: Crossing::Either,
                terminal: true,
                priority: 0,
                boundary: 0.0,
            }
        }
    }

    impl RootEvent<f64> for Level {
        fn value(&self, _t: f64, y: &f64) -> f64 {
            y - self.level
        }
        fn crossing(&self) -> Crossing {
            self.crossing
        }
        fn terminal(&self) -> bool {
            self.terminal
        }
        fn priority(&self) -> i32 {
            self.priority
        }
        fn boundary_tolerance(&self) -> f64 {
            self.boundary
        }
    }

    /// `y' = 1` from `y = 0`, so the raw state after a width is the width
    /// itself. The solution is exact, which makes the located time comparable
    /// against the analytic crossing.
    fn ramp(width: f64) -> Result<f64, IntegrationError> {
        Ok(width)
    }

    /// What a stepper does after it has projected and stored a state: read the
    /// values, then record where the walk is.
    fn commit<const N: usize>(set: &mut RootSet<'_, f64, N>, t: f64, y: &f64) {
        let values = set.check(t, y).expect("finite value");
        set.apply(t, &values);
    }

    #[test]
    fn the_located_time_is_the_analytic_crossing_within_the_tolerance() {
        let level = Level::at(0.25);
        let mut set = RootSet::new(
            [&level as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-9,
                max_iterations: 60,
            },
        )
        .expect("valid search");
        set.begin(0.0, &0.0).expect("finite value");

        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found {
                t,
                state,
                bracket,
                terminal,
            } => {
                assert!((t - 0.25).abs() <= 1e-9, "located t = {t}");
                assert!((state - 0.25).abs() <= 1e-9, "state = {state}");
                assert!(bracket <= 1e-9, "bracket = {bracket}");
                assert!(terminal);
                assert_eq!(
                    set.hits(),
                    &[RootHit {
                        event: 0,
                        terminal: true,
                        priority: 0
                    }]
                );
            }
            StepRoots::None => panic!("the ramp crosses 0.25 inside the step"),
        }
    }

    #[test]
    fn a_level_the_step_does_not_reach_is_not_a_root() {
        let level = Level::at(2.0);
        let mut set =
            RootSet::new([&level as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, ramp).expect("no error"),
            StepRoots::None
        ));
        assert!(set.hits().is_empty());
    }

    /// Two levels inside one step: the search converges on the earlier one, and
    /// the later one is left for the next step.
    #[test]
    fn the_earlier_of_two_crossings_is_the_one_located() {
        let early = Level::at(0.25);
        let late = Level::at(0.75);
        let mut set = RootSet::new(
            [&early as &dyn RootEvent<f64>, &late],
            RootSearch {
                t_tolerance: 1e-9,
                max_iterations: 60,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");

        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, .. } => {
                assert!((t - 0.25).abs() <= 1e-9, "located t = {t}");
                assert_eq!(set.hits().len(), 1, "hits: {:?}", set.hits());
                assert_eq!(set.hits()[0].event, 0);
            }
            StepRoots::None => panic!("both levels are inside the step"),
        }
    }

    /// Two events on the same level cross together, so both are reported, and
    /// priority decides the order rather than registration.
    #[test]
    fn events_crossing_together_are_reported_as_a_group_in_priority_order() {
        let first = Level {
            priority: 5,
            ..Level::at(0.5)
        };
        let second = Level {
            priority: -1,
            terminal: false,
            ..Level::at(0.5)
        };
        let mut set = RootSet::new(
            [&first as &dyn RootEvent<f64>, &second],
            RootSearch::default(),
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");

        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { terminal, .. } => {
                assert!(terminal, "one of the two asked to stop");
                assert_eq!(
                    set.hits(),
                    &[
                        RootHit {
                            event: 1,
                            terminal: false,
                            priority: -1
                        },
                        RootHit {
                            event: 0,
                            terminal: true,
                            priority: 5
                        },
                    ]
                );
            }
            StepRoots::None => panic!("both events are on the level the step crosses"),
        }
    }

    #[test]
    fn a_falling_event_ignores_the_rising_step_that_crosses_its_level() {
        let level = Level {
            crossing: Crossing::Falling,
            ..Level::at(0.5)
        };
        let mut set =
            RootSet::new([&level as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, ramp).expect("no error"),
            StepRoots::None
        ));
    }

    /// After a non-terminal root the state sits on the boundary. The guard is
    /// what keeps the resumed walk from reporting the departure as a crossing,
    /// and it re-arms once the value is clear of the boundary again.
    #[test]
    fn the_walk_resumed_from_a_boundary_does_not_report_it_again() {
        let level = Level {
            terminal: false,
            ..Level::at(0.5)
        };
        let mut set = RootSet::new(
            [&level as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-12,
                max_iterations: 200,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        let t_root = match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, state, .. } => {
                commit(&mut set, t, &state);
                assert!(set.guard(0).is_on_boundary());
                assert_eq!(set.guard(0).at(), Some(t));
                t
            }
            StepRoots::None => panic!("the ramp crosses 0.5"),
        };

        // Resume from the boundary: the value leaves zero upward, which is a
        // departure, not an arrival.
        set.begin(t_root, &0.5).expect("finite value");
        assert!(matches!(
            set.scan_step(t_root, 1.0, &1.5, |w| Ok(0.5 + w))
                .expect("no error"),
            StepRoots::None
        ));
    }

    /// A value that only jitters around zero — the state moving along the
    /// boundary — is not a stream of fresh crossings, which is what the event's
    /// own boundary tolerance settles.
    #[test]
    fn jitter_within_the_boundary_tolerance_is_not_a_new_crossing() {
        let sliding = Level {
            terminal: false,
            boundary: 1e-6,
            ..Level::at(0.0)
        };
        let mut set =
            RootSet::new([&sliding as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        // Reach the boundary from below and commit there, as a non-terminal root
        // leaves the state.
        set.begin(0.0, &-1.0).expect("finite value");
        match set
            .scan_step(0.0, 1.0, &0.0, |w| Ok(-1.0 + w))
            .expect("no error")
        {
            StepRoots::Found { t, state, .. } => {
                commit(&mut set, t, &state);
            }
            StepRoots::None => panic!("the walk reaches zero from below"),
        }
        assert!(set.guard(0).is_on_boundary());

        // Jitter to the other side of zero, well inside the tolerance.
        set.begin(1.0, &-1e-9).expect("finite value");
        assert!(
            set.guard(0).is_on_boundary(),
            "a value inside the tolerance leaves the guard set"
        );
        assert!(matches!(
            set.scan_step(1.0, 1.0, &1e-9, |w| Ok(-1e-9 + 2e-9 * w))
                .expect("no error"),
            StepRoots::None
        ));

        // Once the value is clear of the boundary, the guard re-arms and a
        // return to zero is a crossing again.
        set.begin(2.0, &1.0).expect("finite value");
        assert!(!set.guard(0).is_on_boundary());
        assert!(matches!(
            set.scan_step(2.0, 2.0, &-1.0, |w| Ok(1.0 - w))
                .expect("no error"),
            StepRoots::Found { .. }
        ));
    }

    #[test]
    fn a_non_finite_value_stops_the_walk_rather_than_being_bisected_on() {
        struct Blows;
        impl RootEvent<f64> for Blows {
            fn value(&self, _t: f64, y: &f64) -> f64 {
                if *y > 0.5 { f64::NAN } else { y - 0.75 }
            }
        }
        let event = Blows;
        let mut set =
            RootSet::new([&event as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("the start is finite");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, ramp),
            Err(IntegrationError::NonFiniteRootValue { t, event: 0 }) if t == 1.0
        ));
    }

    /// A value whose sign depends on something other than where the state is —
    /// here on whether the trial reaches the end of the step — has no crossing
    /// for the bisection to narrow. The search reports that rather than a time
    /// it did not localize.
    #[test]
    fn a_search_that_does_not_converge_is_reported_rather_than_rounded() {
        struct Endpoint;
        impl RootEvent<f64> for Endpoint {
            fn value(&self, t: f64, _y: &f64) -> f64 {
                if t >= 1.0 { 1.0 } else { -1.0 }
            }
        }
        let event = Endpoint;
        let mut set = RootSet::new(
            [&event as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-9,
                max_iterations: 4,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, ramp),
            Err(IntegrationError::RootNotLocalized { t, .. }) if t == 0.0
        ));
    }

    #[test]
    fn a_search_that_cannot_narrow_a_bracket_is_refused_before_the_walk() {
        let level = Level::at(1.0);
        for search in [
            RootSearch {
                t_tolerance: 0.0,
                max_iterations: 60,
            },
            RootSearch {
                t_tolerance: -1e-3,
                max_iterations: 60,
            },
            RootSearch {
                t_tolerance: f64::NAN,
                max_iterations: 60,
            },
            RootSearch {
                t_tolerance: f64::INFINITY,
                max_iterations: 60,
            },
            RootSearch {
                t_tolerance: 1e-3,
                max_iterations: 0,
            },
        ] {
            assert!(
                RootSet::new([&level as &dyn RootEvent<f64>], search).is_err(),
                "{search:?} was accepted"
            );
        }
    }

    /// A walk resumed from the state the search actually committed does not
    /// report the same boundary again.
    ///
    /// The search stops on the far side of a bracket, so the value there is a
    /// small non-zero rather than zero — the level is a third, which no bracket
    /// end lands on exactly. What suppresses the report is the time: this step
    /// starts on the root, so whichever way the caller sends the state next,
    /// the sign change it sees belongs to the root already reported.
    #[test]
    fn the_boundary_a_root_left_is_not_reported_again_from_the_state_it_left() {
        let level = Level {
            terminal: false,
            ..Level::at(1.0 / 3.0)
        };
        let mut set = RootSet::new(
            [&level as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-12,
                max_iterations: 200,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        let (t_root, y_root) = match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, state, .. } => {
                commit(&mut set, t, &state);
                (t, state)
            }
            StepRoots::None => panic!("the ramp crosses a third"),
        };
        assert_ne!(
            y_root - 1.0 / 3.0,
            0.0,
            "the committed state is not exactly on the boundary, which is the case \
             a guard reading only the value would miss"
        );

        // Resume from that state with the motion reversed, so the value crosses
        // back through the boundary at once.
        set.begin(t_root, &y_root).expect("finite value");
        assert!(matches!(
            set.scan_step(t_root, 1.0, &(y_root - 1.0), |w| Ok(y_root - w))
                .expect("no error"),
            StepRoots::None
        ));

        // A step that lands away from the boundary clears the guard, and the
        // next return through it is a crossing again.
        let away = y_root + 1.0;
        commit(&mut set, t_root + 1.0, &away);
        assert_eq!(set.guard(0).at(), None);
        set.begin(t_root + 1.0, &away).expect("finite value");
        assert!(matches!(
            set.scan_step(t_root + 1.0, 2.0, &(away - 2.0), |w| Ok(away - w))
                .expect("no error"),
            StepRoots::Found { .. }
        ));
    }

    /// A caller that moves the value across zero before resuming has taken the
    /// state off the root, so the step it then takes reports a crossing.
    ///
    /// The guard suppresses the step leaving a root, and what identifies that
    /// step is the time together with the side the value is on. Suppressing on
    /// the time alone would lose this crossing.
    #[test]
    fn a_value_the_caller_moved_across_zero_crosses_again_from_the_same_time() {
        let level = Level {
            terminal: false,
            crossing: Crossing::Rising,
            ..Level::at(1.0 / 3.0)
        };
        let mut set = RootSet::new(
            [&level as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-12,
                max_iterations: 200,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        let t_root = match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, state, .. } => {
                commit(&mut set, t, &state);
                t
            }
            StepRoots::None => panic!("the ramp crosses a third"),
        };
        assert!(set.guard(0).at() == Some(t_root));

        // The caller puts the state well below the level, at the same time, and
        // resumes. Rising through the level from there is a crossing.
        let below = 0.0;
        set.begin(t_root, &below).expect("finite value");
        assert_eq!(
            set.guard(0).at(),
            Some(t_root),
            "the walk is still standing at that time"
        );
        assert!(matches!(
            set.scan_step(t_root, 1.0, &(below + 1.0), |w| Ok(below + w))
                .expect("no error"),
            StepRoots::Found { .. }
        ));
    }

    /// A root that landed exactly on zero left the value with no side, so a
    /// caller that moves it anywhere non-zero has moved the state off that root.
    ///
    /// `Crossing::Falling` reaching the level from above lands on exactly zero
    /// when the level is a grid time of the walk. Treating zero as the positive
    /// side would then read a return to positive as "still where the root left
    /// it" and suppress the fall that follows.
    #[test]
    fn a_root_that_landed_on_zero_does_not_claim_a_side() {
        let level = Level {
            terminal: false,
            crossing: Crossing::Falling,
            ..Level::at(0.5)
        };
        let mut set = RootSet::new(
            [&level as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-12,
                max_iterations: 200,
            },
        )
        .expect("valid");

        // Fall from 1.0 and land on the level exactly: the value at the root is
        // exactly zero.
        set.begin(0.0, &1.0).expect("finite value");
        let t_root = match set
            .scan_step(0.0, 0.5, &0.5, |w| Ok(1.0 - w))
            .expect("located")
        {
            StepRoots::Found { t, state, .. } => {
                assert_eq!(state, 0.5, "the step lands on the level exactly");
                commit(&mut set, t, &state);
                t
            }
            StepRoots::None => panic!("the fall reaches 0.5"),
        };
        assert_eq!(set.guard(0).at(), Some(t_root));

        // The caller puts the value back above the level, at the same time, and
        // it falls through again. That is a crossing.
        set.begin(t_root, &1.0).expect("finite value");
        assert!(matches!(
            set.scan_step(t_root, 1.0, &0.0, |w| Ok(1.0 - w))
                .expect("no error"),
            StepRoots::Found { .. }
        ));
    }

    /// A walk that starts somewhere other than the last root no longer reports
    /// standing on it.
    #[test]
    fn a_walk_starting_away_from_a_root_forgets_it() {
        let level = Level {
            terminal: false,
            ..Level::at(0.5)
        };
        let mut set =
            RootSet::new([&level as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        let t_root = match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, state, .. } => {
                commit(&mut set, t, &state);
                t
            }
            StepRoots::None => panic!("the ramp crosses 0.5"),
        };
        assert_eq!(set.guard(0).at(), Some(t_root));

        set.begin(t_root + 1.0, &1.5).expect("finite value");
        assert_eq!(set.guard(0).at(), None);
    }

    /// A root the walk gave up on is not left in `hits`.
    ///
    /// The stepper reads the values at the projected state before it moves, and
    /// a non-finite one there fails the walk. The hits the search recorded
    /// belong to a state that was never committed.
    #[test]
    fn hits_are_dropped_when_the_committed_state_has_no_finite_value() {
        /// A pole at a third, which no bisection trial lands on exactly — the
        /// trials are dyadic fractions of the step.
        const POLE: f64 = 1.0 / 3.0;

        struct Breaks;
        impl RootEvent<f64> for Breaks {
            fn value(&self, _t: f64, y: &f64) -> f64 {
                1.0 / (POLE - y)
            }
        }
        let event = Breaks;
        let mut set =
            RootSet::new([&event as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        // A step from 0 to 1 passes the pole, so the value changes sign and the
        // search locates it.
        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { .. } => {
                assert_eq!(set.hits().len(), 1, "the search recorded a hit");
            }
            StepRoots::None => panic!("the value changes sign across the pole"),
        }
        // A projection that put the state exactly on the pole is what the
        // stepper asks about before it moves.
        assert!(matches!(
            set.check(POLE, &POLE),
            Err(IntegrationError::NonFiniteRootValue { .. })
        ));
        assert!(
            set.hits().is_empty(),
            "a hit the walk gave up on stays out of hits(): {:?}",
            set.hits()
        );
    }

    #[test]
    fn an_event_whose_boundary_tolerance_cannot_re_arm_a_guard_is_refused() {
        for bad in [-1e-9, f64::NAN, f64::INFINITY] {
            let event = Level {
                boundary: bad,
                ..Level::at(1.0)
            };
            assert!(
                matches!(
                    RootSet::new([&event as &dyn RootEvent<f64>], RootSearch::default()),
                    Err(IntegrationError::InvalidBoundaryTolerance { event: 0, .. })
                ),
                "a boundary tolerance of {bad} was accepted"
            );
        }
    }

    /// What the search reports when a step holds three crossings of one event,
    /// which the contract asks the caller to prevent by bounding the step.
    ///
    /// The first trial is at the middle of the step, where `g` has the sign it
    /// started with, so the first two crossings are discarded and the bisection
    /// converges on the third. The test states the outcome rather than calling
    /// it correct: a caller who needs the first of three has to give the search
    /// a step that holds one.
    #[test]
    fn three_crossings_in_one_step_converge_on_the_last() {
        struct Cubic;
        impl RootEvent<f64> for Cubic {
            fn value(&self, t: f64, _y: &f64) -> f64 {
                (t - 0.2) * (t - 0.4) * (t - 0.8)
            }
        }
        let event = Cubic;
        let mut set = RootSet::new(
            [&event as &dyn RootEvent<f64>],
            RootSearch {
                t_tolerance: 1e-9,
                max_iterations: 60,
            },
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { t, .. } => {
                assert!((t - 0.8).abs() <= 1e-9, "located {t}");
            }
            StepRoots::None => panic!("the value changes sign over the step"),
        }
    }

    /// Two crossings of one event inside a step cancel, so the step reports
    /// nothing at all.
    #[test]
    fn two_crossings_in_one_step_are_not_seen() {
        struct Dip;
        impl RootEvent<f64> for Dip {
            fn value(&self, t: f64, _y: &f64) -> f64 {
                (t - 0.3) * (t - 0.7)
            }
        }
        let event = Dip;
        let mut set =
            RootSet::new([&event as &dyn RootEvent<f64>], RootSearch::default()).expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, ramp).expect("no error"),
            StepRoots::None
        ));
    }

    /// Events of equal priority are reported in the order they were registered.
    #[test]
    fn events_of_equal_priority_keep_their_registration_order() {
        let first = Level {
            terminal: false,
            ..Level::at(0.5)
        };
        let second = Level {
            terminal: false,
            ..Level::at(0.5)
        };
        let third = Level {
            terminal: false,
            ..Level::at(0.5)
        };
        let mut set = RootSet::new(
            [
                &first as &dyn RootEvent<f64>,
                &second as &dyn RootEvent<f64>,
                &third as &dyn RootEvent<f64>,
            ],
            RootSearch::default(),
        )
        .expect("valid");
        set.begin(0.0, &0.0).expect("finite value");
        match set.scan_step(0.0, 1.0, &1.0, ramp).expect("located") {
            StepRoots::Found { .. } => {
                let order: Vec<usize> = set.hits().iter().map(|hit| hit.event).collect();
                assert_eq!(order, vec![0, 1, 2]);
            }
            StepRoots::None => panic!("all three are on the level the step crosses"),
        }
    }

    /// An empty set makes the walk the plain one: no value evaluation, no trial
    /// step.
    #[test]
    fn a_set_with_no_events_finds_nothing() {
        let mut set: RootSet<'_, f64, 0> = RootSet::new([], RootSearch::default()).expect("valid");
        assert!(set.is_empty());
        set.begin(0.0, &0.0).expect("nothing to evaluate");
        assert!(matches!(
            set.scan_step(0.0, 1.0, &1.0, |_| panic!("no event, so no trial step"))
                .expect("no error"),
            StepRoots::None
        ));
    }
}
