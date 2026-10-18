//! Unity UI (uGUI + TextMesh Pro) scenes from the content pack, rebuilt as Bevy UI.
//!
//! Each Unity node becomes an absolutely positioned Bevy node. RectTransform anchoring is resolved here
//! against the parent's size in canvas units, and the CanvasScaler maps canvas units to the window
//! through `UiScale`. Buttons keep their Unity persistent calls, which `flow` binds to Rust by
//! `Class.Method` name.

use bevy::picking::Pickable;
use bevy::picking::events::{PointerClick, PointerOver};
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};
use bevy::window::PrimaryWindow;
use serde::Deserialize;
use std::collections::HashMap;

use crate::loc::{self, Loc};

#[derive(Deserialize, Clone, Debug)]
pub struct UiDoc {
    pub scene: String,
    pub scaler: Scaler,
    pub canvases: Vec<UiNode>,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Scaler {
    pub reference: [f32; 2],
    #[serde(rename = "match")]
    pub match_height: f32,
    pub mode: i32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct UiNode {
    pub name: String,
    pub active: bool,
    pub rect: Option<URect>,
    pub image: Option<UImage>,
    pub text: Option<UText>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub events: Vec<UEvent>,
    pub alpha: Option<f32>,
    pub canvas: Option<UCanvas>,
    pub layout: Option<ULayout>,
    pub element: Option<UElement>,
    #[serde(default)]
    pub children: Vec<UiNode>,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct URect {
    pub amin: [f32; 2],
    pub amax: [f32; 2],
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub pivot: [f32; 2],
    pub scale: [f32; 2],
}

#[derive(Deserialize, Clone, Debug)]
pub struct UImage {
    pub sprite: Option<USprite>,
    pub color: [f32; 4],
    #[serde(rename = "type")]
    pub kind: i32,
    pub raycast: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct USprite {
    pub file: Option<String>,
    pub border: Option<[f32; 4]>,
    pub ppu: Option<f32>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct UText {
    pub text: String,
    pub size: f32,
    pub color: [f32; 4],
    pub align: i32,
    pub wrap: bool,
    pub font: String,
    pub term: Option<String>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct UEvent {
    pub on: String,
    pub target: String,
    pub method: String,
    pub arg: String,
}

/// Vertical/HorizontalLayoutGroup.
#[derive(Deserialize, Clone, Copy, Debug, Component)]
pub struct ULayout {
    pub axis: char,
    pub padding: [f32; 4], // left, right, top, bottom
    pub spacing: f32,
    pub align: i32,
    pub control: [bool; 2],
    pub expand: [bool; 2],
}

/// LayoutElement (-1 = unset).
#[derive(Deserialize, Clone, Copy, Debug, Component)]
pub struct UElement {
    pub ignore: bool,
    pub min: [f32; 2],
    pub preferred: [f32; 2],
    pub flexible: [f32; 2],
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct UCanvas {
    pub sort: i32,
    #[serde(rename = "override")]
    pub overrides: bool,
}

impl UEvent {
    /// Rollover/pointer-enter calls fire on hover; everything else fires on click.
    pub fn is_hover(&self) -> bool {
        self.on == "UiButtonEventUnityEvent" || self.method.contains("Rollover") || self.method == "OnPointerEnter"
    }

    pub fn key(&self) -> String {
        format!("{}.{}", self.target, self.method)
    }
}

impl UiDoc {
    pub fn load(pack: &std::path::Path, scene: &str) -> Result<UiDoc, String> {
        let file = pack.join("ui").join(format!("{scene}.json"));
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// Every node, depth first, with its path ("Canvas/MainMenu/...").
    pub fn walk(&self) -> Vec<(String, &UiNode)> {
        fn go<'a>(node: &'a UiNode, path: &str, out: &mut Vec<(String, &'a UiNode)>) {
            let path = if path.is_empty() { node.name.clone() } else { format!("{path}/{}", node.name) };
            out.push((path.clone(), node));
            for child in &node.children {
                go(child, &path, out);
            }
        }
        let mut out = Vec::new();
        for canvas in &self.canvases {
            go(canvas, "", &mut out);
        }
        out
    }
}

/// Root of a spawned Unity UI scene.
#[derive(Component)]
pub struct UnityUiRoot {
    pub scene: String,
    pub scaler: Scaler,
}

/// Identity of a spawned Unity node, for lookups by name or script class.
#[derive(Component, Clone, Debug)]
pub struct UnityNode {
    pub name: String,
    pub scripts: Vec<String>,
}

impl UnityNode {
    pub fn has_script(&self, class: &str) -> bool {
        self.scripts.iter().any(|s| s == class)
    }
}

#[derive(Component, Clone, Copy)]
pub struct UnityRect(pub URect);

/// Whether the node is shown; flow logic toggles this, layout turns it into `Display`.
#[derive(Component, Clone, Copy)]
pub struct UnityActive(pub bool);

#[derive(Component, Clone)]
pub struct UnityEvents(pub Vec<UEvent>);

/// A Unity event fired by the player, for `flow` to bind and run.
#[derive(Message, Clone, Debug)]
pub struct UnityCall {
    pub source: Entity,
    pub event: UEvent,
}

/// Font assets by TMP font name, from pack/fonts/fonts.json.
#[derive(Resource, Default)]
pub struct Fonts(pub HashMap<String, Handle<Font>>);

impl Fonts {
    pub fn load(pack: &std::path::Path, assets: &AssetServer) -> Fonts {
        let map: HashMap<String, String> = std::fs::read(pack.join("fonts/fonts.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Fonts(map.into_iter().map(|(name, file)| (name, assets.load(format!("pack://{file}")))).collect())
    }

    pub fn get(&self, name: &str) -> Handle<Font> {
        self.0.get(name).or_else(|| self.0.get("")).cloned().unwrap_or_default()
    }
}

pub struct UnityUiPlugin;

impl Plugin for UnityUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UnityCall>()
            .add_systems(Update, (layout, pointer_events));
    }
}

fn color(c: [f32; 4]) -> Color {
    Color::srgba(c[0], c[1], c[2], c[3])
}

/// Spawn a Unity UI document; returns the root entity.
pub fn spawn(commands: &mut Commands, doc: &UiDoc, assets: &AssetServer, fonts: &Fonts, loc: &Loc) -> Entity {
    let root = commands
        .spawn((
            Node { position_type: PositionType::Absolute, ..default() },
            UnityUiRoot { scene: doc.scene.clone(), scaler: doc.scaler },
            Pickable::IGNORE,
        ))
        .id();
    for canvas in &doc.canvases {
        let child = spawn_node(commands, canvas, assets, fonts, loc, 0, 1.0);
        commands.entity(root).add_child(child);
    }
    root
}

/// `group_alpha` is the product of the CanvasGroup alphas above this node (Unity multiplies them down).
fn spawn_node(
    commands: &mut Commands,
    node: &UiNode,
    assets: &AssetServer,
    fonts: &Fonts,
    loc: &Loc,
    depth: u32,
    group_alpha: f32,
) -> Entity {
    let mut entity = commands.spawn((
        Node { position_type: PositionType::Absolute, ..default() },
        UnityNode { name: node.name.clone(), scripts: node.scripts.clone() },
        UnityActive(node.active),
        Name::new(node.name.clone()),
    ));
    if let Some(rect) = node.rect {
        entity.insert(UnityRect(rect));
    }
    if let Some(layout) = node.layout {
        entity.insert(layout);
    }
    if let Some(element) = node.element {
        entity.insert(element);
    }
    if let Some(canvas) = node.canvas.filter(|c| c.overrides) {
        entity.insert(GlobalZIndex(canvas.sort * 10 + depth as i32));
    }
    let interactive = !node.events.is_empty() || node.image.as_ref().is_some_and(|i| i.raycast && i.color[3] > 0.0);
    if !node.events.is_empty() {
        entity.insert(UnityEvents(node.events.clone()));
    }
    let alpha = group_alpha * node.alpha.unwrap_or(1.0);
    // A fully transparent group is how Unity hides panels until a script fades them in.
    let (interactive, hidden_group) = (interactive && alpha > 0.0, alpha <= 0.0);
    if let Some(image) = &node.image {
        let tint = color([image.color[0], image.color[1], image.color[2], image.color[3] * alpha]);
        match image.sprite.as_ref().and_then(|s| s.file.as_ref().map(|f| (s, f))) {
            Some((sprite, file)) => {
                let mut image_node = ImageNode::new(assets.load(format!("pack://{file}"))).with_color(tint);
                image_node.image_mode = match (image.kind, sprite.border) {
                    (1, Some(b)) if b.iter().any(|v| *v > 0.0) => {
                        // Unity draws borders at sprite pixels / (ppu / 100) canvas units.
                        let k = 100.0 / sprite.ppu.unwrap_or(100.0).max(1.0);
                        NodeImageMode::Sliced(TextureSlicer {
                            border: BorderRect { min_inset: Vec2::new(b[0], b[3]) * k, max_inset: Vec2::new(b[2], b[1]) * k },
                            center_scale_mode: SliceScaleMode::Stretch,
                            sides_scale_mode: SliceScaleMode::Stretch,
                            max_corner_scale: 1.0,
                        })
                    }
                    _ => NodeImageMode::Stretch,
                };
                entity.insert(image_node);
            }
            None if image.color[3] > 0.0 => {
                entity.insert(BackgroundColor(tint));
            }
            None => {}
        }
    }
    entity.insert(if interactive { Pickable::default() } else { Pickable::IGNORE });
    // UIPanel (the game's popup base class) starts closed until game logic opens it.
    if hidden_group || node.scripts.iter().any(|s| s == "UIPanel") {
        entity.insert(UnityActive(false));
    }
    let id = entity.id();
    if let Some(text) = &node.text {
        let content = text.term.as_deref().and_then(|t| loc.get(t)).unwrap_or(&text.text);
        let (justify_content, align_items, justify) = text_alignment(text.align);
        commands.entity(id).insert(Node {
            position_type: PositionType::Absolute,
            display: Display::Flex,
            justify_content,
            align_items,
            ..default()
        });
        let label = commands
            .spawn((
                Text::new(loc::plain(content)),
                TextFont {
                    font: FontSource::Handle(fonts.get(&text.font)),
                    font_size: FontSize::from(text.size.max(1.0)),
                    ..default()
                },
                TextColor(color([text.color[0], text.color[1], text.color[2], text.color[3] * alpha])),
                TextLayout::new(justify, if text.wrap { LineBreak::WordBoundary } else { LineBreak::NoWrap }),
                Node { max_width: Val::Percent(100.0), ..default() },
                Pickable::IGNORE,
            ))
            .id();
        commands.entity(id).add_child(label);
    }
    for child in &node.children {
        let child = spawn_node(commands, child, assets, fonts, loc, depth + 1, alpha);
        commands.entity(id).add_child(child);
    }
    id
}

/// TMP alignment flags to (horizontal flex, vertical flex, text justify).
fn text_alignment(align: i32) -> (JustifyContent, AlignItems, Justify) {
    let (justify_content, justify) = match align & 0xff {
        2 | 32 => (JustifyContent::Center, Justify::Center),
        4 => (JustifyContent::FlexEnd, Justify::Right),
        8 | 16 => (JustifyContent::FlexStart, Justify::Justified),
        _ => (JustifyContent::FlexStart, Justify::Left),
    };
    let align_items = match align & 0xff00 {
        512 | 4096 => AlignItems::Center,
        1024 | 2048 => AlignItems::FlexEnd,
        _ => AlignItems::FlexStart,
    };
    (justify_content, align_items, justify)
}

/// The rect of a node inside its parent, in canvas units, top-left based: (left, top, width, height).
pub fn resolve(rect: &URect, parent: Vec2) -> (f32, f32, f32, f32) {
    let (a0x, a1x) = (rect.amin[0] * parent.x, rect.amax[0] * parent.x);
    let (a0y, a1y) = (rect.amin[1] * parent.y, rect.amax[1] * parent.y);
    let w = (a1x - a0x) + rect.size[0];
    let h = (a1y - a0y) + rect.size[1];
    let left = a0x + (a1x - a0x) * rect.pivot[0] + rect.pos[0] - w * rect.pivot[0];
    let bottom = a0y + (a1y - a0y) * rect.pivot[1] + rect.pos[1] - h * rect.pivot[1];
    (left, parent.y - bottom - h, w, h)
}

/// CanvasScaler: canvas units per logical pixel for a window size.
pub fn scale_factor(scaler: &Scaler, window: Vec2) -> f32 {
    let (rw, rh) = (scaler.reference[0].max(1.0), scaler.reference[1].max(1.0));
    let (sx, sy) = (window.x / rw, window.y / rh);
    match scaler.mode {
        1 => sx.min(sy),
        2 => sx.max(sy),
        _ => (sx.ln() * (1.0 - scaler.match_height) + sy.ln() * scaler.match_height).exp(),
    }
}

#[allow(clippy::type_complexity)]
fn layout(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut ui_scale: ResMut<UiScale>,
    roots: Query<(Entity, &UnityUiRoot)>,
    children: Query<&Children>,
    mut nodes: Query<NodeParts>,
    changed: Query<(), Or<(Added<UnityRect>, Changed<UnityActive>, Changed<UnityRect>)>>,
    mut last: Local<Vec2>,
) {
    let Ok(window) = windows.single() else { return };
    let size = Vec2::new(window.width(), window.height());
    if size == *last && changed.is_empty() {
        return;
    }
    *last = size;
    for (root, ui) in &roots {
        let scale = scale_factor(&ui.scaler, size);
        ui_scale.0 = scale;
        let canvas = size / scale;
        if let Ok((mut node, ..)) = nodes.get_mut(root) {
            node.left = Val::Px(0.0);
            node.top = Val::Px(0.0);
            node.width = Val::Px(canvas.x);
            node.height = Val::Px(canvas.y);
        }
        // Top-level canvases fill the screen whatever their serialized rect says.
        for canvas_entity in children.get(root).into_iter().flat_map(|c| c.iter()) {
            place(canvas_entity, canvas, Placement::Fill, &children, &mut nodes);
        }
    }
}

type NodeParts = (
    &'static mut Node,
    Option<&'static UnityRect>,
    Option<&'static UnityActive>,
    Option<&'static ULayout>,
    Option<&'static UElement>,
);

/// How a node gets its rect: filling its parent (top-level canvas), from its own RectTransform, or
/// from its parent's layout group.
#[derive(Clone, Copy)]
enum Placement {
    Fill,
    Own,
    Driven(f32, f32, f32, f32),
}

fn shown(rect: Option<&UnityRect>, active: Option<&UnityActive>) -> bool {
    active.is_none_or(|a| a.0) && rect.is_none_or(|r| r.0.scale[0] != 0.0 && r.0.scale[1] != 0.0)
}

fn place(entity: Entity, parent: Vec2, placement: Placement, children: &Query<&Children>, nodes: &mut Query<NodeParts>) {
    let mut own = parent;
    let mut layout = None;
    if let Ok((mut node, rect, active, group, _)) = nodes.get_mut(entity) {
        node.display = if shown(rect, active) { Display::Flex } else { Display::None };
        layout = group.copied();
        let resolved = match (placement, rect) {
            (Placement::Driven(l, t, w, h), _) => Some((l, t, w, h)),
            (Placement::Fill, Some(_)) => Some((0.0, 0.0, parent.x, parent.y)),
            (Placement::Own, Some(rect)) => Some(resolve(&rect.0, parent)),
            (_, None) => None,
        };
        if let Some((left, top, w, h)) = resolved {
            node.left = Val::Px(left);
            node.top = Val::Px(top);
            node.width = Val::Px(w.max(0.0));
            node.height = Val::Px(h.max(0.0));
            own = Vec2::new(w, h);
        }
    }
    let kids: Vec<Entity> = children.get(entity).into_iter().flat_map(|c| c.iter()).collect();
    let driven = layout.map(|group| layout_group(&group, own, &kids, nodes)).unwrap_or_default();
    for child in kids {
        let placement = driven.iter().find(|(e, _)| *e == child).map_or(Placement::Own, |(_, p)| *p);
        place(child, own, placement, children, nodes);
    }
}

/// Unity's Horizontal/VerticalLayoutGroup: stack the active, non-ignoring children along the axis.
fn layout_group(group: &ULayout, size: Vec2, kids: &[Entity], nodes: &mut Query<NodeParts>) -> Vec<(Entity, Placement)> {
    let main = if group.axis == 'v' { 1 } else { 0 };
    let cross = 1 - main;
    let [pl, pr, pt, pb] = group.padding;
    let inner = Vec2::new(size.x - pl - pr, size.y - pt - pb);
    let mut items = Vec::new();
    for &kid in kids {
        let Ok((_, rect, active, _, element)) = nodes.get(kid) else { continue };
        if !shown(rect, active) || element.is_some_and(|e| e.ignore) {
            continue;
        }
        let base = rect.map_or([0.0, 0.0], |r| r.0.size);
        let preferred = |axis: usize| element.map(|e| e.preferred[axis]).filter(|v| *v >= 0.0).unwrap_or(base[axis]);
        let mut extent = [0.0; 2];
        for axis in 0..2 {
            extent[axis] = if group.control[axis] { preferred(axis) } else { base[axis] };
        }
        if group.control[cross] && group.expand[cross] {
            extent[cross] = inner[cross];
        }
        items.push((kid, extent));
    }
    let total: f32 = items.iter().map(|(_, e)| e[main]).sum::<f32>() + group.spacing * items.len().saturating_sub(1) as f32;
    // childAlignment: 0..8 = Upper/Middle/Lower x Left/Center/Right.
    let (row, column) = (group.align / 3, group.align % 3);
    let main_fraction = if main == 1 { row } else { column } as f32 / 2.0;
    let cross_fraction = if main == 1 { column } else { row } as f32 / 2.0;
    let mut cursor = (inner[main] - total) * main_fraction;
    let origin = Vec2::new(pl, pt);
    let mut out = Vec::new();
    for (kid, extent) in items {
        let mut pos = [0.0; 2];
        pos[main] = origin[main] + cursor;
        pos[cross] = origin[cross] + (inner[cross] - extent[cross]) * cross_fraction;
        out.push((kid, Placement::Driven(pos[0], pos[1], extent[0], extent[1])));
        cursor += extent[main] + group.spacing;
    }
    out
}

/// Clicks and hovers on Unity buttons become `UnityCall` messages (the hit may be a child, so walk up).
fn pointer_events(
    mut clicks: MessageReader<PointerClick>,
    mut overs: MessageReader<PointerOver>,
    parents: Query<&ChildOf>,
    events: Query<&UnityEvents>,
    mut calls: MessageWriter<UnityCall>,
) {
    let mut fire = |hit: Entity, hover: bool| {
        let mut current = Some(hit);
        while let Some(entity) = current {
            if let Ok(list) = events.get(entity) {
                let matching: Vec<_> = list.0.iter().filter(|e| e.is_hover() == hover).cloned().collect();
                if !matching.is_empty() {
                    for event in matching {
                        calls.write(UnityCall { source: entity, event });
                    }
                    return;
                }
            }
            current = parents.get(entity).ok().map(ChildOf::parent);
        }
    };
    for click in clicks.read() {
        fire(click.entity, false);
    }
    for over in overs.read() {
        fire(over.entity, true);
    }
}
