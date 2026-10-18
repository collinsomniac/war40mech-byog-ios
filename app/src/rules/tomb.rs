//! Tomb generation (TaskExploGenerateDungeon + TaskExploSetupEvents): rooms scattered and pushed
//! apart, linked by a spanning tree over their Delaunay triangulation, each room using at most one door
//! per side, plus a few loops; then the mission's events are placed by distance from the start room.

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

use super::rng::{Rng, round};
use super::save::Save;
use crate::data::{Db, Record, Ref, items};

const ROOM_SIZE: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    Top,
    Left,
    Right,
    Bottom,
}

/// Edge.GetDirection: the side of `from` a corridor to `to` leaves by.
pub fn side(from: Vec2, to: Vec2) -> Side {
    let d = to - from;
    let angle = d.y.atan2(d.x).to_degrees();
    if angle > -45.0 && angle <= 45.0 {
        Side::Right
    } else if angle > 45.0 && angle <= 135.0 {
        Side::Bottom
    } else if angle > 135.0 || angle <= -135.0 {
        Side::Left
    } else {
        Side::Top
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub pos: Vec2,
    /// Corridors from this room, as indices into `Tomb::links`.
    pub links: Vec<usize>,
    /// Steps from the start room.
    pub depth: usize,
    pub event: Option<Ref>,
    /// The mission objective this room's event counts towards.
    pub objective: bool,
    pub glyphs: Vec<Ref>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Link {
    pub a: usize,
    pub b: usize,
}

impl Link {
    pub fn other(&self, room: usize) -> usize {
        if self.a == room { self.b } else { self.a }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tomb {
    pub rooms: Vec<Room>,
    pub links: Vec<Link>,
    pub start: usize,
    pub seed: i32,
}

impl Tomb {
    pub fn neighbours(&self, room: usize) -> impl Iterator<Item = usize> + '_ {
        self.rooms[room].links.iter().map(move |&l| self.links[l].other(room))
    }

    pub fn deepest(&self) -> usize {
        self.rooms.iter().map(|r| r.depth).max().unwrap_or(0)
    }
}

/// The Mission fields generation reads.
pub struct MissionSpec<'a> {
    pub record: &'a Record,
    pub min_rooms: i32,
    pub straight_line: bool,
    pub is_tutorial: bool,
    pub is_sector_boss: bool,
    pub min_auto_fights: i32,
    pub min_glyph_events: i32,
    pub limit_extra_fight: bool,
    pub extra_fight_max: i32,
    pub objective_events: Vec<&'a Record>,
    pub custom_events: Vec<&'a Record>,
    pub seed: Option<i32>,
}

impl<'a> MissionSpec<'a> {
    pub fn read(db: &'a Db, mission: &'a Record) -> MissionSpec<'a> {
        let objective_events = mission
            .get("primaryObjective")
            .map(|o| mission.deref(o))
            .and_then(|o| o.get("completeSpecificsEvents"))
            .map(|o| mission.deref(o))
            .and_then(|o| o.get("dungeonEvents"))
            .map(|list| items(mission.deref(list)).iter().filter_map(|e| db.resolve(mission, e)).collect())
            .unwrap_or_default();
        let custom_events = mission
            .get("customEvents")
            .map(|list| items(mission.deref(list)).iter().filter_map(|e| db.resolve(mission, e)).collect())
            .unwrap_or_default();
        MissionSpec {
            record: mission,
            min_rooms: mission.int("minNbRooms") as i32,
            straight_line: mission.flag("straightLine"),
            is_tutorial: mission.flag("isTutorial"),
            is_sector_boss: mission.flag("isSectorBoss"),
            min_auto_fights: mission.int("minAutoFights") as i32,
            min_glyph_events: mission.int("minGlyphEvents") as i32,
            limit_extra_fight: mission.flag("limitExtraFight"),
            extra_fight_max: mission.int("extraFightMax") as i32,
            objective_events,
            custom_events,
            seed: mission.flag("overrideSeed").then(|| mission.int("seed") as i32),
        }
    }

    /// Mission.GetNbFights: fight events the mission already places itself.
    fn fights(&self) -> i32 {
        self.objective_events.iter().chain(&self.custom_events).filter(|e| e.int("type") == 3).count() as i32
    }
}

/// GameplayConfig.GetAdditionalFights: later in the campaign, tombs hold more fights.
fn additional_fights(save: &Save, mission: &Record) -> i32 {
    let over = save.story.clamped_countdown() - mission.float("necronAwakeningRequirement");
    if over > 0.0 { ((over / 10.0).floor() as i32).clamp(0, 3) } else { 0 }
}

pub fn generate(db: &Db, save: &Save, mission: &Record, seed: Option<i32>) -> Result<Tomb, String> {
    let spec = MissionSpec::read(db, mission);
    let seed = seed.or(spec.seed).unwrap_or_else(Rng::any_seed);
    let mut rng = Rng::new(seed);

    let mut rooms = spec.min_rooms;
    let min_rooms = spec.objective_events.len() as i32 + 1;
    if !save.settings.narrative_event {
        rooms -= spec.custom_events.len() as i32;
    }
    if !save.settings.glyph_event {
        rooms -= spec.min_glyph_events;
    }
    rooms = rooms.max(min_rooms);
    let (additional, extra) = (additional_fights(save, mission), save.settings.extra_fights());
    let mut total_extra = additional + extra;
    if spec.limit_extra_fight {
        total_extra = total_extra.clamp(0, spec.extra_fight_max);
    }
    rooms += total_extra;

    for _attempt in 0..200 {
        let (positions, edges) = layout(&mut rng, rooms.max(2) as usize, spec.straight_line);
        let mut tomb = Tomb {
            rooms: positions.iter().map(|&pos| Room { pos, links: Vec::new(), depth: 0, event: None, objective: false, glyphs: Vec::new() }).collect(),
            links: Vec::new(),
            start: 0,
            seed,
        };
        let mut sorted = edges;
        sorted.sort_by(|&(a, b), &(c, d)| positions[a].distance(positions[b]).total_cmp(&positions[c].distance(positions[d])));
        for (a, b) in sorted {
            let index = tomb.links.len();
            tomb.links.push(Link { a, b });
            tomb.rooms[a].links.push(index);
            tomb.rooms[b].links.push(index);
        }
        if place_events(db, save, &spec, &mut tomb, &mut rng, additional, extra) {
            return Ok(tomb);
        }
    }
    Err(format!("mission {}: no tomb layout could hold its events", mission.name()))
}

/// DelaunayGeneration: room centres and the corridors between them.
fn layout(rng: &mut Rng, n: usize, straight: bool) -> (Vec<Vec2>, Vec<(usize, usize)>) {
    loop {
        let positions = scatter(rng, n, straight);
        let edges = if straight { line(&positions) } else { connect(rng, &positions) };
        if let Some(edges) = edges {
            return (positions, edges);
        }
    }
}

/// Cells start near the origin and push each other apart until none overlap, then snap to the grid.
fn scatter(rng: &mut Rng, n: usize, straight: bool) -> Vec<Vec2> {
    let mut cells: Vec<Vec2> = (0..n).map(|_| Vec2::new(rng.range_f(-ROOM_SIZE, ROOM_SIZE), rng.range_f(-ROOM_SIZE, ROOM_SIZE))).collect();
    rng.shuffle(&mut cells);
    let overlaps = |a: Vec2, b: Vec2| (a.x - b.x).abs() < ROOM_SIZE && (a.y - b.y).abs() < ROOM_SIZE;
    for _ in 0..10_000 {
        let mut moved = false;
        for i in 0..n {
            let mut push = Vec2::ZERO;
            for j in 0..n {
                if i == j || !overlaps(cells[i], cells[j]) {
                    continue;
                }
                // Cell.Move: away from the overlapping cell, but never back towards the origin (the other
                // cell makes that move instead).
                let mut away = (cells[i] - cells[j]).normalize_or_zero();
                if away == Vec2::ZERO {
                    away = cells[i].normalize_or_zero();
                }
                if away == Vec2::ZERO {
                    away = Vec2::new(rng.range_f(-1.0, 1.0), rng.range_f(-1.0, 1.0)).normalize_or_zero();
                }
                if away.dot(cells[i]) < 0.0 {
                    continue;
                }
                if !straight {
                    push.x += away.x;
                }
                push.y += away.y;
            }
            if push != Vec2::ZERO {
                cells[i] += push.normalize_or_zero();
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    cells.iter().map(|c| Vec2::new(round(c.x) as f32, round(c.y) as f32)).collect()
}

/// Straight-line missions: rooms chained in order of distance from the farthest one.
fn line(p: &[Vec2]) -> Option<Vec<(usize, usize)>> {
    let first = (0..p.len()).max_by(|&a, &b| p[a].length().total_cmp(&p[b].length()))?;
    let mut order: Vec<usize> = (0..p.len()).collect();
    order.sort_by(|&a, &b| p[first].distance(p[a]).total_cmp(&p[first].distance(p[b])));
    Some(order.windows(2).map(|w| (w[0], w[1])).collect())
}

/// Spanning tree over the Delaunay triangulation, one corridor per room side, then up to n/2 loops.
fn connect(rng: &mut Rng, p: &[Vec2]) -> Option<Vec<(usize, usize)>> {
    let n = p.len();
    let candidates = delaunay(p);
    let mut edges = prim(p, &candidates);
    // A room can't use the same side twice: drop the later corridor and reconnect any islands.
    let mut kept: Vec<(usize, usize)> = Vec::new();
    for &(a, b) in &edges {
        if side_free(p, &kept, a, b) {
            kept.push((a, b));
        }
    }
    edges = kept;
    let mut guard = 0;
    while !connected(n, &edges) {
        guard += 1;
        if guard > n * 2 {
            return None;
        }
        let reach = reachable(n, &edges, 0);
        let mut best: Option<(usize, usize)> = None;
        for &a in &reach {
            for b in 0..n {
                if reach.contains(&b) || !side_free(p, &edges, a, b) || crosses(p, &edges, a, b) {
                    continue;
                }
                if best.is_none_or(|(x, y)| p[a].distance(p[b]) < p[x].distance(p[y])) {
                    best = Some((a, b));
                }
            }
        }
        edges.push(best?);
    }
    // AddAdditionalEdges: a few loops, from shuffled rooms, to the nearest room they can reach cleanly.
    let mut order: Vec<usize> = (0..n).collect();
    rng.shuffle(&mut order);
    let mut added = 0;
    for &a in &order {
        if added >= n / 2 {
            break;
        }
        let best = (0..n)
            .filter(|&b| b != a && !edges.iter().any(|&(x, y)| (x, y) == (a, b) || (y, x) == (a, b)))
            .filter(|&b| side_free(p, &edges, a, b) && !crosses(p, &edges, a, b))
            .min_by(|&x, &y| p[a].distance(p[x]).total_cmp(&p[a].distance(p[y])));
        if let Some(b) = best {
            edges.push((a, b));
            added += 1;
        }
    }
    Some(edges)
}

fn side_free(p: &[Vec2], edges: &[(usize, usize)], a: usize, b: usize) -> bool {
    let used = |room: usize| edges.iter().filter_map(move |&(x, y)| if x == room { Some(y) } else if y == room { Some(x) } else { None });
    let sa = side(p[a], p[b]);
    let sb = side(p[b], p[a]);
    used(a).all(|o| side(p[a], p[o]) != sa) && used(b).all(|o| side(p[b], p[o]) != sb)
}

fn crosses(p: &[Vec2], edges: &[(usize, usize)], a: usize, b: usize) -> bool {
    edges.iter().any(|&(x, y)| x != a && x != b && y != a && y != b && segments_cross(p[a], p[b], p[x], p[y]))
}

fn segments_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    let orient = |p: Vec2, q: Vec2, r: Vec2| (q - p).perp_dot(r - p);
    let (d1, d2, d3, d4) = (orient(c, d, a), orient(c, d, b), orient(a, b, c), orient(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

fn reachable(n: usize, edges: &[(usize, usize)], from: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(r) = queue.pop_front() {
        for &(a, b) in edges {
            let other = if a == r { b } else if b == r { a } else { continue };
            if other < n && seen.insert(other) {
                queue.push_back(other);
            }
        }
    }
    seen
}

fn connected(n: usize, edges: &[(usize, usize)]) -> bool {
    reachable(n, edges, 0).len() == n
}

/// Bowyer–Watson triangulation; returns its unique edges.
fn delaunay(p: &[Vec2]) -> Vec<(usize, usize)> {
    let n = p.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![(0, 1)];
    }
    let (min, max) = p.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), &q| (lo.min(q), hi.max(q)));
    let span = (max - min).max_element().max(1.0) * 20.0;
    let mid = (min + max) / 2.0;
    let mut pts = p.to_vec();
    pts.push(mid + Vec2::new(-span, -span));
    pts.push(mid + Vec2::new(span, -span));
    pts.push(mid + Vec2::new(0.0, span));
    let mut tris: Vec<[usize; 3]> = vec![[n, n + 1, n + 2]];
    let circum = |t: &[usize; 3], q: Vec2| -> bool {
        let (a, b, c) = (pts[t[0]] - q, pts[t[1]] - q, pts[t[2]] - q);
        let det = (a.length_squared()) * b.perp_dot(c) - (b.length_squared()) * a.perp_dot(c) + (c.length_squared()) * a.perp_dot(b);
        let orient = (pts[t[1]] - pts[t[0]]).perp_dot(pts[t[2]] - pts[t[0]]);
        if orient > 0.0 { det > 0.0 } else { det < 0.0 }
    };
    for i in 0..n {
        let q = pts[i];
        let (bad, good): (Vec<[usize; 3]>, Vec<[usize; 3]>) = tris.iter().partition(|t| circum(t, q));
        let mut boundary: Vec<(usize, usize)> = Vec::new();
        for t in &bad {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let shared = bad.iter().filter(|o| *o != t).any(|o| o.contains(&a) && o.contains(&b));
                if !shared {
                    boundary.push((a, b));
                }
            }
        }
        tris = good;
        tris.extend(boundary.into_iter().map(|(a, b)| [a, b, i]));
    }
    let mut out = BTreeSet::new();
    for t in &tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if a < n && b < n {
                out.insert((a.min(b), a.max(b)));
            }
        }
    }
    out.into_iter().collect()
}

fn prim(p: &[Vec2], edges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let n = p.len();
    let mut inside = vec![false; n];
    inside[0] = true;
    let mut tree = Vec::new();
    for _ in 1..n {
        let next = edges
            .iter()
            .filter(|&&(a, b)| inside[a] != inside[b])
            .min_by(|&&(a, b), &&(c, d)| p[a].distance(p[b]).total_cmp(&p[c].distance(p[d])));
        let Some(&(a, b)) = next else { break };
        inside[a] = true;
        inside[b] = true;
        tree.push((a, b));
    }
    tree
}

/// TaskExploSetupEvents. False when this layout can't hold the mission's events (generation retries).
fn place_events(db: &Db, save: &Save, spec: &MissionSpec, tomb: &mut Tomb, rng: &mut Rng, additional: i32, extra: i32) -> bool {
    let Some(config) = db.single("DungeonEventConfig") else { return false };
    // FindStartRoom: the room farthest from the centroid.
    let centroid = tomb.rooms.iter().map(|r| r.pos).sum::<Vec2>() / tomb.rooms.len() as f32;
    let Some(start) = (0..tomb.rooms.len()).max_by(|&a, &b| tomb.rooms[a].pos.distance(centroid).total_cmp(&tomb.rooms[b].pos.distance(centroid)))
    else {
        return false;
    };
    tomb.start = start;
    let starts = db.follow_all(config, "startEvents");
    let Some(start_event) = rng.pick(&starts) else { return false };
    tomb.rooms[start].event = Some(start_event.id);
    depths(tomb);

    if !place_by_position(tomb, rng, &spec.objective_events, true) {
        return false;
    }
    if save.settings.narrative_event && !place_by_position(tomb, rng, &spec.custom_events, false) {
        return false;
    }
    let mut fights = spec.min_auto_fights + extra + (additional - spec.fights() + 1).max(0);
    if spec.limit_extra_fight {
        fights = fights.clamp(0, spec.extra_fight_max);
    }
    if !spec.is_sector_boss && !spec.is_tutorial {
        let pool = db.follow_all(config, "fightEvents");
        for _ in 0..fights {
            let Some(event) = rng.pick(&pool).map(|e| e.id) else { break };
            if !place_anywhere(tomb, rng, event, Vec::new()) {
                break;
            }
        }
    }
    if spec.min_glyph_events > 0 && save.settings.glyph_event {
        let countdown = save.story.clamped_countdown();
        let events: Vec<&Record> = db.follow_all(config, "glyphEvents").into_iter().filter(|e| e.float("percentAvailable") <= countdown).collect();
        let glyphs: Vec<&Record> = db.follow_all(config, "glyphs").into_iter().filter(|g| g.float("percentAvailable") <= countdown).collect();
        for _ in 0..spec.min_glyph_events {
            let Some(event) = rng.pick(&events) else { break };
            let mut pool = glyphs.clone();
            let mut chosen = Vec::new();
            for _ in 0..event.int("nbGlyphs") {
                if pool.is_empty() {
                    break;
                }
                let i = rng.range(0, pool.len() as i32) as usize;
                chosen.push(pool.remove(i).id);
            }
            if !place_anywhere(tomb, rng, event.id, chosen) {
                break;
            }
        }
    }
    true
}

fn depths(tomb: &mut Tomb) {
    let n = tomb.rooms.len();
    let mut depth = vec![usize::MAX; n];
    depth[tomb.start] = 0;
    let mut queue = VecDeque::from([tomb.start]);
    while let Some(r) = queue.pop_front() {
        let next: Vec<usize> = tomb.neighbours(r).collect();
        for o in next {
            if depth[o] == usize::MAX {
                depth[o] = depth[r] + 1;
                queue.push_back(o);
            }
        }
    }
    for (room, d) in tomb.rooms.iter_mut().zip(depth) {
        room.depth = d;
    }
}

/// TaskExploEventObjective.AddEvents: tightest position constraints first, each at a depth its
/// `rangePosition` allows (as a fraction of the deepest room).
fn place_by_position(tomb: &mut Tomb, rng: &mut Rng, events: &[&Record], objective: bool) -> bool {
    let mut order: Vec<&&Record> = events.iter().collect();
    let width = |e: &Record| -> f32 {
        if !e.flag("constrainPosition") {
            return 1.0;
        }
        let r = e.get("rangePosition");
        let x = r.and_then(|r| r.get("x")).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let y = r.and_then(|r| r.get("y")).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
        y - x
    };
    order.sort_by(|a, b| width(a).total_cmp(&width(b)));
    let deepest = tomb.deepest();
    for event in order {
        let mut positions = room_positions(event, deepest);
        rng.shuffle(&mut positions);
        let mut placed = false;
        'search: for depth in positions {
            let mut rooms: Vec<usize> = (0..tomb.rooms.len()).filter(|&r| tomb.rooms[r].depth == depth).collect();
            rng.shuffle(&mut rooms);
            for r in rooms {
                if tomb.rooms[r].event.is_none() {
                    tomb.rooms[r].event = Some(event.id);
                    tomb.rooms[r].objective = objective;
                    placed = true;
                    break 'search;
                }
            }
        }
        if !placed {
            return false;
        }
    }
    true
}

/// DungeonEvent.GetRandomRoomPositions.
fn room_positions(event: &Record, deepest: usize) -> Vec<usize> {
    if !event.flag("constrainPosition") {
        return (1..deepest).collect();
    }
    let range = event.get("rangePosition");
    let x = range.and_then(|r| r.get("x")).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let y = range.and_then(|r| r.get("y")).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
    let start = ((deepest as f32 * x).floor() as usize).max(1);
    let end = ((deepest as f32 * y).ceil() as usize).max(1);
    (start..=end).collect()
}

fn place_anywhere(tomb: &mut Tomb, rng: &mut Rng, event: Ref, glyphs: Vec<Ref>) -> bool {
    let mut rooms: Vec<usize> = (0..tomb.rooms.len()).collect();
    rng.shuffle(&mut rooms);
    for r in rooms {
        if tomb.rooms[r].event.is_none() {
            tomb.rooms[r].event = Some(event);
            tomb.rooms[r].glyphs = glyphs;
            return true;
        }
    }
    false
}
