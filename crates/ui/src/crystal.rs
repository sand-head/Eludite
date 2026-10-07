//! Eludite's mark drawn as a 3D object: a translucent crystal cube with an amber core fixed at its center, lit from a
//! fixed light at the upper right (the light does not turn with the cube; each face is shaded from its normal after
//! rotation). At rest it holds the isometric pose of the flat mark (`tools/package/icons/eludite.svg`).
//!
//! Motion: the Welcome page's open plays once (the cube enters spinning, four turns, and eases out onto the isometric
//! pose in 1.6 s); while the IDE builds or a debuggee runs it spins on Y (one turn per 2.4 s) with a Z roll (one turn per
//! 5.2 s) and the core pulses; when that ends each axis eases out from the spin speed onto the next equivalent
//! isometric pose in about 1 to 1.6 s.
//!
//! Public API: [`Motion`], the pose over time (pure, no GPUI); [`Crystal`], which keeps a `Motion` and draws it,
//! asking for the next animation frame only while the cube moves.

use std::cell::RefCell;
use std::f32::consts::PI;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    Bounds, IntoElement, ParentElement, PathBuilder, Pixels, Rgba, Styled, Window, canvas, div,
    point, px,
};

/// The isometric pitch: the cube's body diagonal points at the viewer.
pub const ISO_PITCH: f32 = -35.264;
/// The isometric yaw.
pub const ISO_YAW: f32 = 45.;
/// Spin speeds while busy, in degrees per millisecond.
const SPIN_YAW: f32 = 0.15;
const SPIN_ROLL: f32 = 0.07;
/// The Welcome open's length, in milliseconds.
const INTRO_MS: f32 = 1600.;
/// The core's pulse period while busy, in milliseconds.
const PULSE_MS: f32 = 1200.;

/// A pose in degrees, applied as CSS applies `rotateX(pitch) rotateY(yaw) rotateZ(roll)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl Pose {
    pub const ISOMETRIC: Pose = Pose {
        pitch: ISO_PITCH,
        yaw: ISO_YAW,
        roll: 0.,
    };
}

/// One axis easing out: a cubic that starts at the spin speed and ends at rest, so the stop has no jolt.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ease {
    from: f32,
    by: f32,
    ms: f32,
    speed: f32,
}

/// The shortest and longest stop, in milliseconds.
const STOP_MS: (f32, f32) = (900., 1600.);

impl Ease {
    /// The stop for an axis at `at` turning at `speed` (degrees per millisecond): onto the nearest pose equivalent to
    /// `base` (the cube repeats every 90 degrees) far enough ahead that an ease-out from `speed` takes at least 900
    /// ms. A plain cubic ease-out covers `by` in `3 * by / speed`; when that is longer than 1.6 s the stop takes 1.6 s
    /// and eases harder, never turning back (`by` is at least `speed * ms / 3`).
    fn plan(at: f32, speed: f32, base: f32) -> Self {
        let least = speed * STOP_MS.0 / 3.;
        let k = ((at + least - base) / 90.).ceil();
        let by = base + k * 90. - at;
        Ease {
            from: at,
            by,
            ms: (3. * by / speed).min(STOP_MS.1),
            speed,
        }
    }

    /// Where the axis is `t` milliseconds into the stop: the cubic Hermite from (`from`, `speed`) to
    /// (`from + by`, 0).
    fn at(&self, t: f32) -> f32 {
        let u = (t / self.ms).min(1.);
        let reach = self.speed * self.ms;
        self.from + self.by * (3. * u * u - 2. * u * u * u) + reach * (u - 2. * u * u + u * u * u)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Rest,
    Intro { t: f32 },
    Spin { t: f32 },
    Stopping { t: f32, yaw: Ease, roll: Ease },
}

/// The cube's pose over time. Advance it by the time since the last frame and whether the IDE is busy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    yaw: f32,
    roll: f32,
    state: State,
}

impl Default for Motion {
    fn default() -> Self {
        Self::rest()
    }
}

impl Motion {
    /// At rest on the isometric pose.
    pub fn rest() -> Self {
        Self {
            yaw: ISO_YAW,
            roll: 0.,
            state: State::Rest,
        }
    }

    /// About to play the Welcome open.
    pub fn intro() -> Self {
        Self {
            state: State::Intro { t: 0. },
            ..Self::rest()
        }
    }

    /// True while the pose changes from frame to frame.
    pub fn moving(&self) -> bool {
        self.state != State::Rest
    }

    /// True while spinning for a build or a debuggee.
    pub fn spinning(&self) -> bool {
        matches!(self.state, State::Spin { .. })
    }

    /// Moves `ms` milliseconds on.
    pub fn advance(&mut self, ms: f32, busy: bool) {
        // A long frame (the window was hidden) moves at most 64 ms, so a stop is never skipped.
        let ms = ms.clamp(0., 64.);
        self.state = match self.state {
            _ if busy => {
                let t = match self.state {
                    State::Spin { t } => t + ms,
                    State::Intro { .. } => {
                        // Spin from where the open is, so the cube does not jump.
                        let pose = self.pose();
                        self.yaw = pose.yaw;
                        self.roll = pose.roll;
                        0.
                    }
                    _ => 0.,
                };
                self.yaw += ms * SPIN_YAW;
                self.roll += ms * SPIN_ROLL;
                State::Spin { t }
            }
            State::Rest => State::Rest,
            State::Intro { t } if t + ms >= INTRO_MS => State::Rest,
            State::Intro { t } => State::Intro { t: t + ms },
            State::Spin { .. } => State::Stopping {
                t: 0.,
                yaw: Ease::plan(self.yaw, SPIN_YAW, ISO_YAW),
                roll: Ease::plan(self.roll, SPIN_ROLL, 0.),
            },
            State::Stopping { t, yaw, roll } => {
                let t = t + ms;
                self.yaw = yaw.at(t);
                self.roll = roll.at(t);
                if t >= yaw.ms.max(roll.ms) {
                    self.yaw = ISO_YAW;
                    self.roll = 0.;
                    State::Rest
                } else {
                    State::Stopping { t, yaw, roll }
                }
            }
        };
    }

    /// The pose to draw now.
    pub fn pose(&self) -> Pose {
        match self.state {
            State::Intro { t } => {
                let e = (1. - (t / INTRO_MS).min(1.)).powi(3);
                Pose {
                    pitch: ISO_PITCH + 40. * e,
                    yaw: ISO_YAW - 1440. * e,
                    roll: -90. * e,
                }
            }
            _ => Pose {
                pitch: ISO_PITCH,
                yaw: self.yaw,
                roll: self.roll,
            },
        }
    }

    /// The core's brightness: 1 at rest, pulsing up to 1.6 while spinning.
    pub fn glow(&self) -> f32 {
        match self.state {
            State::Spin { t } => 1. + 0.3 * (1. - (2. * PI * t / PULSE_MS).cos()),
            _ => 1.,
        }
    }
}

/// The light, in view space (x right, y down, z toward the viewer): from the upper right and slightly in front.
const LIGHT: [f32; 3] = [0.447, -0.844, 0.298];

/// A shading ramp: (facing the light, color, alpha) at the shadow, middle and lit stops.
type Ramp = [(f32, [f32; 3], f32); 3];

/// The shell: dark teal in shadow, teal between, light yellow where lit; translucent so the core shows.
const SHELL: Ramp = [
    (-0.49, [10., 84., 80.], 0.8),
    (0.14, [16., 165., 126.], 0.55),
    (0.86, [255., 212., 92.], 0.6),
];
/// The core: opaque amber.
const CORE: Ramp = [
    (-0.49, [181., 127., 24.], 1.),
    (0.14, [201., 142., 30.], 1.),
    (0.86, [224., 165., 43.], 1.),
];

fn shade(d: f32, ramp: &Ramp, glow: f32) -> Rgba {
    let (rgb, a) = if d <= ramp[0].0 {
        (ramp[0].1, ramp[0].2)
    } else if d >= ramp[2].0 {
        (ramp[2].1, ramp[2].2)
    } else {
        let (p, q) = if d < ramp[1].0 {
            (ramp[0], ramp[1])
        } else {
            (ramp[1], ramp[2])
        };
        let t = (d - p.0) / (q.0 - p.0);
        let mix = |i: usize| p.1[i] + (q.1[i] - p.1[i]) * t;
        ([mix(0), mix(1), mix(2)], p.2 + (q.2 - p.2) * t)
    };
    let c = |v: f32| (v * glow / 255.).min(1.);
    Rgba {
        r: c(rgb[0]),
        g: c(rgb[1]),
        b: c(rgb[2]),
        a,
    }
}

/// Rotates a vector by `pose`.
fn rotate(v: [f32; 3], pose: Pose) -> [f32; 3] {
    let (sa, ca) = pose.pitch.to_radians().sin_cos();
    let (sb, cb) = pose.yaw.to_radians().sin_cos();
    let (sg, cg) = pose.roll.to_radians().sin_cos();
    let [x0, y0, z] = v;
    let (x, y) = (x0 * cg - y0 * sg, x0 * sg + y0 * cg);
    let (x1, z1) = (x * cb + z * sb, -x * sb + z * cb);
    let (y2, z2) = (y * ca - z1 * sa, y * sa + z1 * ca);
    [x1, y2, z2]
}

/// A cube's faces: the outward normal and the corners, each a sign per axis.
const FACES: [([f32; 3], [[f32; 3]; 4]); 6] = [
    (
        [0., 0., 1.],
        [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
    ),
    (
        [0., 0., -1.],
        [
            [1., -1., -1.],
            [-1., -1., -1.],
            [-1., 1., -1.],
            [1., 1., -1.],
        ],
    ),
    (
        [-1., 0., 0.],
        [
            [-1., -1., -1.],
            [-1., -1., 1.],
            [-1., 1., 1.],
            [-1., 1., -1.],
        ],
    ),
    (
        [1., 0., 0.],
        [[1., -1., 1.], [1., -1., -1.], [1., 1., -1.], [1., 1., 1.]],
    ),
    (
        [0., -1., 0.],
        [
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., -1., 1.],
            [-1., -1., 1.],
        ],
    ),
    (
        [0., 1., 0.],
        [[-1., 1., 1.], [1., 1., 1.], [1., 1., -1.], [-1., 1., -1.]],
    ),
];

/// A face to fill: its corners as offsets from the center, in units of the shell's half edge, and its color.
#[derive(Debug, Clone, PartialEq)]
pub struct Facet {
    pub corners: [[f32; 2]; 4],
    pub color: Rgba,
}

/// The faces facing the viewer, in paint order: the core's, then the shell's over them.
pub fn facets(pose: Pose, glow: f32) -> Vec<Facet> {
    let mut out = Vec::with_capacity(6);
    for (scale, ramp, glow) in [(0.5, &CORE, glow), (1., &SHELL, 1.)] {
        for (normal, corners) in FACES {
            let n = rotate(normal, pose);
            if n[2] <= 1e-4 {
                continue;
            }
            let d = n[0] * LIGHT[0] + n[1] * LIGHT[1] + n[2] * LIGHT[2];
            out.push(Facet {
                corners: corners.map(|c| {
                    let p = rotate(c, pose);
                    [p[0] * scale, p[1] * scale]
                }),
                color: shade(d, ramp, glow),
            });
        }
    }
    out
}

/// The mark in 3D, `edge` pixels along the shell's edge, in a square box that holds it in every pose.
#[derive(Debug, Clone)]
pub struct Crystal {
    edge: Pixels,
    state: Rc<RefCell<(Motion, Option<Instant>)>>,
}

impl Crystal {
    /// At rest.
    pub fn new(edge: Pixels) -> Self {
        Self::with_motion(edge, Motion::rest())
    }

    /// Starting with the Welcome open.
    pub fn opening(edge: Pixels) -> Self {
        Self::with_motion(edge, Motion::intro())
    }

    fn with_motion(edge: Pixels, motion: Motion) -> Self {
        Self {
            edge,
            state: Rc::new(RefCell::new((motion, None))),
        }
    }

    /// Plays the Welcome open again.
    pub fn replay(&self) {
        *self.state.borrow_mut() = (Motion::intro(), None);
    }

    pub fn motion(&self) -> Motion {
        self.state.borrow().0
    }

    /// The side of the box the cube is drawn in: its body diagonal, so no pose leaves it.
    pub fn box_size(&self) -> Pixels {
        self.edge * 3f32.sqrt()
    }

    /// Advances the motion to now and draws it. While the cube moves, asks for another frame, which redraws the view
    /// being rendered.
    pub fn render(&self, busy: bool, window: &mut Window) -> impl IntoElement + use<> {
        let (pose, glow) = {
            let mut s = self.state.borrow_mut();
            let now = Instant::now();
            // The first frame of a motion starts its clock.
            let ms =
                s.1.map_or(0., |last| now.duration_since(last).as_secs_f32() * 1000.);
            s.0.advance(ms, busy);
            s.1 = s.0.moving().then_some(now);
            if s.0.moving() {
                window.request_animation_frame();
            }
            (s.0.pose(), s.0.glow())
        };
        let half = f32::from(self.edge) / 2.;
        let size = self.box_size();
        div().size(size).flex_none().child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let c = bounds.center();
                    for f in facets(pose, glow) {
                        let mut path = PathBuilder::fill();
                        let at = |p: [f32; 2]| point(c.x + px(p[0] * half), c.y + px(p[1] * half));
                        path.move_to(at(f.corners[0]));
                        for p in &f.corners[1..] {
                            path.line_to(at(*p));
                        }
                        path.close();
                        if let Ok(path) = path.build() {
                            window.paint_path(path, f.color);
                        }
                    }
                },
            )
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn rest_shows_three_shell_faces_lit_as_the_flat_mark() {
        let f = facets(Pose::ISOMETRIC, 1.);
        // Three core faces, then three shell faces over them.
        assert_eq!(f.len(), 6);
        let shell = &f[3..];
        // The flat mark's top, left and right walls: light yellow at 60%, dark teal at 80%, teal at 55%.
        let lit = shell.iter().find(|f| near(f.color.a, 0.6)).unwrap();
        assert!(near(lit.color.r, 1.) && near(lit.color.g, 212. / 255.));
        let dark = shell.iter().find(|f| near(f.color.a, 0.8)).unwrap();
        assert!(near(dark.color.g, 84. / 255.));
        assert!(shell.iter().any(|f| near(f.color.a, 0.55)));
        // The core is opaque amber and half the shell's size.
        assert!(f[..3].iter().all(|f| f.color.a == 1.));
        let extent = |fs: &[Facet]| {
            fs.iter()
                .flat_map(|f| f.corners)
                .map(|p| p[1].abs())
                .fold(0., f32::max)
        };
        assert!(near(extent(&f[..3]) * 2., extent(shell)));
    }

    #[test]
    fn the_light_stays_put_as_the_cube_turns() {
        // A quarter turn about Y brings an equivalent pose: the same three shades on screen.
        let a = facets(Pose::ISOMETRIC, 1.);
        let b = facets(
            Pose {
                yaw: ISO_YAW + 90.,
                ..Pose::ISOMETRIC
            },
            1.,
        );
        let mut sa: Vec<_> = a
            .iter()
            .map(|f| (f.color.a * 100.).round() as i32)
            .collect();
        let mut sb: Vec<_> = b
            .iter()
            .map(|f| (f.color.a * 100.).round() as i32)
            .collect();
        sa.sort();
        sb.sort();
        assert_eq!(sa, sb);
    }

    #[test]
    fn the_open_ends_on_the_isometric_pose_within_two_seconds() {
        let mut m = Motion::intro();
        assert!(m.moving());
        // It starts away from the pose.
        assert!(m.pose().yaw < -1000.);
        let mut ms = 0.;
        while m.moving() {
            m.advance(16., false);
            ms += 16.;
            assert!(ms < 2000., "the open is 2 s at most");
        }
        assert_eq!(m.pose(), Pose::ISOMETRIC);
    }

    #[test]
    fn spinning_turns_yaw_and_roll_and_pulses_the_core() {
        let mut m = Motion::rest();
        assert!(!m.moving());
        m.advance(16., true);
        m.advance(600., true);
        assert!(m.spinning());
        let p = m.pose();
        assert!(p.yaw > ISO_YAW && p.roll > 0.);
        assert_eq!(p.pitch, ISO_PITCH);
        let glows: Vec<f32> = (0..20)
            .map(|_| {
                m.advance(60., true);
                m.glow()
            })
            .collect();
        assert!(glows.iter().any(|g| *g > 1.5));
    }

    #[test]
    fn a_stop_eases_onto_an_equivalent_isometric_pose_without_a_jolt() {
        let mut m = Motion::rest();
        for _ in 0..77 {
            m.advance(16., true);
        }
        let before = m.pose();
        m.advance(16., false);
        let mut prev = m.pose();
        let mut ms = 16.;
        // The first stopping step moves at about the spin speed, not faster.
        m.advance(16., false);
        assert!((m.pose().yaw - prev.yaw) <= 16. * SPIN_YAW * 1.1);
        assert!(m.pose().yaw >= before.yaw);
        while m.moving() {
            prev = m.pose();
            m.advance(16., false);
            ms += 16.;
            // Never turns backwards.
            assert!(m.pose().yaw >= prev.yaw - 1e-3 || !m.moving());
        }
        assert!((900. ..=1700.).contains(&ms), "{ms}");
        assert_eq!(m.pose(), Pose::ISOMETRIC);
    }

    #[test]
    fn every_stop_lands_within_the_window_and_never_turns_back() {
        for speed in [SPIN_YAW, SPIN_ROLL] {
            for tenth in 0..3600 {
                let at = tenth as f32 / 10.;
                let e = Ease::plan(at, speed, 45.);
                assert!(
                    (STOP_MS.0 - 0.5..=STOP_MS.1).contains(&e.ms),
                    "{at} {speed}: {}",
                    e.ms
                );
                assert!(near(
                    (e.from + e.by - 45.)
                        .rem_euclid(90.)
                        .min(90. - (e.from + e.by - 45.).rem_euclid(90.)),
                    0.
                ));
                let mut prev = e.at(0.);
                for step in 1..=100 {
                    let p = e.at(e.ms * step as f32 / 100.);
                    assert!(p >= prev - 1e-3, "{at} {speed} turns back");
                    prev = p;
                }
                assert!(near(prev, e.from + e.by));
            }
        }
    }

    #[test]
    fn busy_during_the_open_spins_from_where_the_cube_is() {
        let mut m = Motion::intro();
        m.advance(16., false);
        m.advance(200., false);
        let at = m.pose();
        m.advance(0., true);
        assert!(m.spinning());
        assert!(near(m.pose().yaw, at.yaw));
    }

    #[test]
    fn long_frames_are_clamped() {
        let mut m = Motion::intro();
        m.advance(10_000., false);
        assert!(m.moving());
    }
}
