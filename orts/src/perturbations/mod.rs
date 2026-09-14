mod constant_thrust;
mod drag;
mod srp;
mod third_body;
mod zonal_gravity;

// The co-rotating atmosphere turns with the frame chain, so the default
// `omega_body` is the rate that chain rotates at rather than the geodetic
// nominal constant.
pub use arika::earth::ERA_RATE as OMEGA_EARTH;
pub use constant_thrust::ConstantThrust;
pub use drag::{AtmosphericDrag, DEFAULT_BALLISTIC_COEFF};
pub use srp::{
    CENTRAL_SHADOW_MODEL, DEFAULT_AREA_TO_MASS, DEFAULT_CR, SOLAR_RADIATION_PRESSURE,
    SolarRadiationPressure, SunPositionFn,
};
pub use third_body::{BodyPositionFn, ThirdBodyGravity};
pub use zonal_gravity::ZonalGravity;
