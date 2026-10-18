//! The game's camera rig (BulwarkStudios.Engine3D.CameraContainer): a container on the floor, a pivot
//! pitched and turned (pitch eases towards `xRotationZoom` as the view zooms in), and the camera on the
//! pivot's local z at the zoom distance. Each scene's own settings come from the pack (battle rooms carry
//! theirs; the tomb table's is camera/Exploration.json). One finger (or the left mouse button) drags the
//! container across the floor within the room's bounds; two fingers pinch (or the wheel) zoom. A press that
//! turned into a drag doesn't also count as a tap.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::input::touch::Touches;
use bevy::prelude::*;
use serde::Deserialize;

use crate::MainCamera;

/// Pixels a press may travel and still be a tap.
const TAP_SLOP: f32 = 14.0;

/// A CameraContainer's settings and state, in Unity's coordinates.
#[derive(Resource, Clone, Debug, Deserialize)]
pub struct Rig {
    /// Container position (the floor point the view centres on).
    pub position: [f32; 3],
    /// Pivot euler angles: pitch, yaw, roll (degrees).
    pub pivot_euler: [f32; 3],
    /// Camera local z: negative distance from the pivot.
    pub zoom: f32,
    pub fov: f32,
    pub zoom_min: f32,
    pub zoom_max: f32,
    #[serde(default)]
    pub x_rotation_zoom: f32,
    /// Floor bounds for the container: [[min x, y, min z], [max x, y, max z]].
    #[serde(default)]
    pub bounds: Option<[[f32; 3]; 2]>,
    /// Where the container is heading (StepCameraCenter), when set.
    #[serde(skip)]
    pub target: Option<[f32; 3]>,
}

impl Rig {
    pub fn load(path: &std::path::Path) -> Option<Rig> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// CameraContainer.UpdateRotation: x = Lerp(baseX, xRotationZoom, InverseLerp(zoomMax, zoomMin, zoom)).
    fn pitch(&self) -> f32 {
        let span = self.zoom_min - self.zoom_max;
        let t = if span.abs() < 1e-3 { 0.0 } else { ((self.zoom - self.zoom_max) / span).clamp(0.0, 1.0) };
        self.pivot_euler[0] + (self.x_rotation_zoom - self.pivot_euler[0]) * t
    }

    /// Unity rotation of the pivot (Euler order Z, X, Y).
    fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.pivot_euler[1].to_radians(), self.pitch().to_radians(), self.pivot_euler[2].to_radians())
    }

    /// Unity-space horizontal axes of the view (right, forward), for dragging.
    fn axes(&self) -> (Vec3, Vec3) {
        let yaw = Quat::from_rotation_y(self.pivot_euler[1].to_radians());
        (yaw * Vec3::X, yaw * Vec3::Z)
    }

    fn clamp(&mut self) {
        if let Some([lo, hi]) = self.bounds {
            self.position[0] = self.position[0].clamp(lo[0], hi[0]);
            self.position[2] = self.position[2].clamp(lo[2], hi[2]);
        }
        let (lo, hi) = (self.zoom_min.min(self.zoom_max), self.zoom_min.max(self.zoom_max));
        self.zoom = self.zoom.clamp(lo, hi);
    }

    /// The camera's Bevy transform (x mirrored like every converted position; Unity cameras look down +z).
    pub fn transform(&self) -> Transform {
        let rot = self.rotation();
        let c = Vec3::from(self.position);
        let unity_pos = c + rot * Vec3::new(0.0, 0.0, self.zoom);
        let pos = Vec3::new(-unity_pos.x, unity_pos.y, unity_pos.z);
        let q = Quat::from_xyzw(rot.x, -rot.y, -rot.z, rot.w) * Quat::from_rotation_y(std::f32::consts::PI);
        Transform::from_translation(pos).with_rotation(q)
    }
}

#[derive(Resource, Default)]
pub struct CameraDrag {
    travelled: f32,
    /// The press just released had been a drag: ignore its click.
    pub suppress_click: bool,
    /// The player moved the view this frame (screens stop auto-centring).
    pub moved: bool,
    pinch: Option<f32>,
}

/// Input → rig, then rig → camera.
pub fn control(
    mut drag: ResMut<CameraDrag>,
    rig: Option<ResMut<Rig>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    touches: Res<Touches>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    windows: Query<&Window>,
    time: Res<Time>,
) {
    let Some(mut rig) = rig else { return };
    let height = windows.iter().next().map(|w| w.height()).unwrap_or(800.0).max(1.0);
    drag.suppress_click = false;
    drag.moved = false;
    let mut pan = Vec2::ZERO;
    let mut zoom = 0.0;
    let active: Vec<_> = touches.iter().collect();
    if active.len() >= 2 {
        let d = active[0].position().distance(active[1].position());
        if let Some(prev) = drag.pinch {
            zoom += (d - prev) / height * 40.0;
        }
        drag.pinch = Some(d);
        drag.travelled = f32::MAX;
    } else {
        drag.pinch = None;
        if let Some(touch) = active.first() {
            pan += touch.delta();
        }
        if mouse.pressed(MouseButton::Left) {
            pan += motion.delta;
        }
    }
    zoom += scroll.delta.y * 2.0;
    if mouse.pressed(MouseButton::Left) || !active.is_empty() {
        drag.travelled += pan.length();
    }
    if mouse.just_released(MouseButton::Left) || touches.iter_just_released().next().is_some() {
        drag.suppress_click = drag.travelled > TAP_SLOP;
        drag.travelled = 0.0;
    }
    if drag.travelled > TAP_SLOP && pan != Vec2::ZERO {
        // Move the floor with the finger: world units per pixel at the current distance.
        let per_pixel = -rig.zoom * 2.0 * (rig.fov.to_radians() / 2.0).tan() / height;
        let (right, forward) = rig.axes();
        // Unity x is mirrored on screen, so a rightward drag moves the container towards Unity +x.
        let delta = right * pan.x * per_pixel + forward * pan.y * per_pixel;
        rig.position[0] += delta.x;
        rig.position[2] += delta.z;
        rig.target = None;
        drag.moved = true;
    }
    if zoom != 0.0 {
        rig.zoom += zoom;
        drag.moved = true;
    }
    // StepCameraCenter: glide to a requested floor point.
    if let Some(target) = rig.target {
        let t = (time.delta_secs() * 4.0).min(1.0);
        for i in 0..3 {
            rig.position[i] += (target[i] - rig.position[i]) * t;
        }
        if (0..3).all(|i| (target[i] - rig.position[i]).abs() < 0.01) {
            rig.target = None;
        }
    }
    rig.clamp();
    if let Ok((mut t, mut projection)) = camera.single_mut() {
        *t = rig.transform();
        if let Projection::Perspective(p) = projection.as_mut() {
            p.fov = rig.fov.to_radians();
        }
    }
}
