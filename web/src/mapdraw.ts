// The map's canvas (RFC 0028): a 32 x 20 tile village, one building per
// component, and homie walking between them. Pixel art is drawn in code at
// 1x and scaled by CSS. The palette here is the one place DESIGN.md allows
// decorative color; status uses only the status colors.

import type { Tone } from "./ui";

export type Building = "pve" | "hist" | "k8s" | "flux" | "graf" | "bell" | "gh" | "logs" | "well" | "upd" | "sec" | "cert" | "bao" | "ts";
export type Look = { tone: Tone; n: number };
export type Looks = Partial<Record<Building, Look>>;

const T = 16, W = 32, H = 20;
export const MAP_W = W * T, MAP_H = H * T;

/** Footprint in tiles; `door` is the walkable tile in front of it. */
export const PLOTS: Record<Building, { x: number; y: number; w: number; h: number; door: [number, number] }> = {
  pve: { x: 1, y: 2, w: 5, h: 5, door: [3, 7] },
  hist: { x: 13, y: 2, w: 5, h: 5, door: [15, 7] },
  k8s: { x: 19, y: 2, w: 5, h: 5, door: [21, 7] },
  flux: { x: 25, y: 2, w: 3, h: 5, door: [26, 7] },
  graf: { x: 29, y: 2, w: 3, h: 5, door: [30, 7] },
  bell: { x: 12, y: 7, w: 1, h: 2, door: [12, 9] },
  gh: { x: 1, y: 11, w: 5, h: 4, door: [3, 15] },
  logs: { x: 7, y: 11, w: 5, h: 4, door: [9, 15] },
  well: { x: 12, y: 11, w: 2, h: 2, door: [14, 12] },
  upd: { x: 12, y: 13, w: 2, h: 2, door: [12, 15] },
  sec: { x: 18, y: 11, w: 3, h: 4, door: [19, 15] },
  cert: { x: 23, y: 12, w: 3, h: 3, door: [24, 15] },
  bao: { x: 27, y: 11, w: 5, h: 4, door: [29, 15] },
  ts: { x: 15, y: 18, w: 2, h: 2, door: [15, 18] },
};
const PEN = { x: 6, y: 2, w: 6, h: 6 };
const DOG = { x: 21, y: 13, w: 2, h: 2 };

// the dark-theme values of the status tokens: they read on grass in both themes
const TONE: Record<Tone, string> = { ok: "#4cc38a", warn: "#f0b44c", bad: "#f2666a", quiet: "#8a8f98", muted: "#62666d" };

const C = {
  grass: "#78bf4f", grass2: "#70b548", tuft: "#5c9e3a", flowerA: "#f6f0d8", flowerB: "#f2c14e",
  path: "#dcc08a", pathEdge: "#c2a46c", pebble: "#c7aa74",
  water: "#4a9fd6", water2: "#9fd3f2", waterDeep: "#3b86ba",
  leaf: "#3f8a3a", leafHi: "#5aa64a", leafDk: "#2c6b2a", trunk: "#6b4226",
  wood: "#a0703f", woodDk: "#6b4226", outline: "#2a1e16",
  barn: "#b8452f", barnDk: "#8f3322", trim: "#f4efe2", roof: "#6b4a3a", roofDk: "#4d3428",
  wall: "#efe3c4", wallDk: "#d6c59c", stone: "#a9a9a1", stoneDk: "#7d7d76",
  glass: "#bfe6f0", glassHi: "#e8f7fb", frame: "#e9ecef",
  crop: "#4fa83f", cropHi: "#86cf5c", cropSick: "#c9b04a", cropDead: "#8a6a3a",
  slate: "#3f6f8f", slateDk: "#2f5670", win: "#ffd76a", winDk: "#3a4a5a",
  metal: "#c3cad4", metalDk: "#8a929c", hill: "#68b046", hillHi: "#82c65a", gold: "#e2b23c",
  homieRoof: "#4f55d6", homieRoofHi: "#7c83f0", homieRoofDk: "#353aa6", brick: "#a2553a",
};
const hash = (x: number, y: number) => ((x * 73856093) ^ (y * 19349663)) >>> 0;
type Px = (x: number, y: number, w: number, h: number, c: string) => void;
const pen = (ctx: CanvasRenderingContext2D): Px => (x, y, w, h, c) => {
  ctx.fillStyle = c;
  ctx.fillRect(x, y, w, h);
};

// ---------- homie, 16 x 21 ----------
const K = "#1e1b3a", SKIN = "#f3c89c", SKINDK = "#dca277", HAIR = "#6b4226", SHIRT = "#f5f1e6",
  OVR = "#3f45c2", OVRDK = "#2b2f8f", BOOT = "#5a3a22", METAL = "#c3cad4", CHEEK = "#f09a8a", WIN = "#ffd76a";
export type Dir = "down" | "up" | "left" | "right";
export type Mood = "sweat" | "sleep" | null;

/** homie at (ox, oy), top left; frames 0-3 are stand, step, stand, step. */
export function drawHomie(ctx: CanvasRenderingContext2D, ox: number, oy: number, dir: Dir, frame: number, mood: Mood) {
  const q = pen(ctx);
  ctx.save();
  if (dir === "left") {
    ctx.translate(ox + 16, oy);
    ctx.scale(-1, 1);
  } else ctx.translate(ox, oy);
  const side = dir === "left" || dir === "right", back = dir === "up";
  const step = frame % 2 === 1, leftStep = frame === 1;
  q(3, 20, 10, 1, "rgba(0,0,0,.25)");
  q(4, 19, 8, 1, "rgba(0,0,0,.15)");
  // legs and boots stay put while the body bobs
  if (!side) {
    q(5, 18, 2, 1, OVRDK);
    q(9, 18, 2, 1, OVRDK);
    q(4, step && leftStep ? 18 : 19, 3, 1, BOOT);
    q(9, step && !leftStep ? 18 : 19, 3, 1, BOOT);
  } else if (!step) {
    q(6, 18, 4, 1, OVRDK);
    q(6, 19, 5, 1, BOOT);
  } else {
    q(4, 18, 2, 1, OVRDK);
    q(9, 18, 2, 1, OVRDK);
    q(3, 19, 3, 1, BOOT);
    q(9, 19, 3, 1, BOOT);
  }
  ctx.translate(0, step ? 1 : 0);
  // the cap: a house roof with a chimney and a lit window
  q(7, 0, 2, 1, K);
  for (let i = 0; i < 6; i++) q(6 - i, 1 + i, 4 + 2 * i, 1, K);
  for (let i = 0; i < 6; i++) {
    q(7 - i, 1 + i, 2 + 2 * i, 1, C.homieRoof);
    if (i) q(7 - i, 1 + i, i, 1, C.homieRoofHi);
  }
  q(1, 7, 14, 1, C.homieRoofDk);
  q(0, 7, 1, 1, K);
  q(15, 7, 1, 1, K);
  const chx = back || side ? 4 : 10;
  q(chx - 1, 0, 4, 4, K);
  q(chx, 1, 2, 3, C.brick);
  if (!back) q(side ? 9 : 7, 4, 2, 2, WIN);
  // head
  q(3, 8, 1, 5, K);
  q(12, 8, 1, 5, K);
  if (back) {
    q(4, 8, 8, 5, HAIR);
    q(6, 12, 4, 1, SKINDK);
  } else if (side) {
    q(4, 8, 8, 5, SKIN);
    q(4, 8, 3, 4, HAIR);
    q(4, 8, 8, 1, HAIR);
    q(10, 10, 1, 2, K);
    q(9, 12, 1, 1, CHEEK);
    q(12, 10, 1, 2, SKIN);
    q(13, 10, 1, 2, K);
  } else {
    q(4, 8, 8, 5, SKIN);
    q(4, 8, 8, 1, HAIR);
    q(4, 9, 1, 2, HAIR);
    q(11, 9, 1, 2, HAIR);
    if (mood === "sleep") {
      q(5, 10, 2, 1, K);
      q(9, 10, 2, 1, K);
    } else {
      q(6, 10, 1, 2, K);
      q(9, 10, 1, 2, K);
    }
    q(5, 12, 1, 1, CHEEK);
    q(10, 12, 1, 1, CHEEK);
    q(7, 12, 2, 1, SKINDK);
  }
  // body
  q(3, 13, 1, 5, K);
  q(12, 13, 1, 5, K);
  q(4, 13, 8, 1, SHIRT);
  q(4, 14, 8, 4, OVR);
  q(4, 17, 8, 1, OVRDK);
  if (!back) {
    q(5, 13, 1, 1, OVR);
    q(10, 13, 1, 1, OVR);
    q(7, 14, 2, 2, OVRDK);
    q(5, 14, 1, 1, WIN);
    q(10, 14, 1, 1, WIN);
  } else {
    q(5, 13, 1, 2, OVRDK);
    q(10, 13, 1, 2, OVRDK);
    q(6, 15, 4, 1, OVRDK);
  }
  // arms and the wrench
  if (!side) {
    const wr = back ? 1 : 13;
    q(wr, 13, 1, 6, METAL);
    q(wr - 1, 12, 3, 2, METAL);
    q(wr, 12, 1, 1, K);
    q(1, 14, 1, 3, K);
    q(14, 14, 1, 3, K);
    q(2, 14, 2, 2, SHIRT);
    q(12, 14, 2, 2, SHIRT);
    q(2, 16, 2, 1, SKIN);
    q(12, 16, 2, 1, SKIN);
  } else {
    const ax = frame === 1 ? 9 : frame === 3 ? 5 : 7;
    q(ax - 1, 14, 4, 3, K);
    q(ax, 14, 2, 2, SHIRT);
    q(ax, 16, 2, 1, SKIN);
    q(ax + 2, 16, 4, 1, METAL);
    q(ax + 5, 15, 2, 3, METAL);
    q(ax + 6, 16, 1, 1, K);
  }
  if (mood === "sweat") {
    q(13, 9, 1, 2, "#9fd3f2");
    q(13, 11, 1, 1, "#e8f7fb");
  }
  ctx.restore();
}

// ---------- the world ----------
export type MapHandle = { set(looks: Looks, quiet: boolean): void; stop(): void };

export function startMap(cv: HTMLCanvasElement, on: { near(b: Building | null): void; open(b: Building): void }, reduce: boolean): MapHandle {
  cv.width = MAP_W;
  cv.height = MAP_H;
  const g = cv.getContext("2d")!;
  const p = pen(g);
  let looks: Looks = {};
  let quiet = false;
  let solid: boolean[][] = [];
  let bg: HTMLCanvasElement | null = null;
  const lights: [number, number, number, number][] = [];
  const puffs: { x: number; y: number; life: number }[] = [];
  const has = (b: Building) => !!looks[b];
  const built = () => (Object.keys(PLOTS) as Building[]).filter(has);

  function rebuild() {
    const ground = Array.from({ length: H }, () => Array<string>(W).fill("g"));
    solid = Array.from({ length: H }, () => Array<boolean>(W).fill(false));
    const trees: [number, number][] = [];
    for (let x = 0; x < W; x++) {
      ground[9][x] = ground[10][x] = ground[16][x] = ground[17][x] = "p";
      ground[18][x] = ground[19][x] = "w";
      solid[18][x] = solid[19][x] = true;
      trees.push([x, 0]);
    }
    for (let y = 11; y < 18; y++) ground[y][15] = ground[y][16] = "p";
    // no Tailscale, no bridge: the village has no way out
    if (has("ts")) for (const y of [18, 19]) for (const x of [15, 16]) (ground[y][x] = "b"), (solid[y][x] = false);
    for (const y of [1, 3, 5, 7]) trees.push([0, y]);
    if (!has("gh")) for (const y of [12, 14]) trees.push([0, y]);
    const blocks = (b: { x: number; y: number; w: number; h: number }) => {
      for (let y = b.y; y < b.y + b.h; y++) for (let x = b.x; x < b.x + b.w; x++) solid[y][x] = true;
    };
    blocks(PLOTS.hist); // homie's house stands with or without the history
    for (const id of built()) {
      if (id === "ts") continue;
      const b = PLOTS[id];
      blocks(b);
      const [dx, dy] = b.door;
      if (dy === 7) ground[7][dx] = ground[8][dx] = "p";
      else if (ground[dy][dx] === "g" && id !== "well") ground[dy][dx] = "p";
    }
    ground[7][15] = ground[8][15] = "p";
    if (has("sec")) blocks(DOG);
    if (has("pve"))
      for (let y = PEN.y; y < PEN.y + PEN.h; y++)
        for (let x = PEN.x; x < PEN.x + PEN.w; x++) if (x === PEN.x || y === PEN.y || x === PEN.x + PEN.w - 1 || y === PEN.y + PEN.h - 1) solid[y][x] = true;
    for (const [x, y] of trees) solid[y][x] = true;

    bg = document.createElement("canvas");
    bg.width = MAP_W;
    bg.height = MAP_H;
    const q = pen(bg.getContext("2d")!);
    const isPath = (x: number, y: number) => y < 0 || y >= H || x < 0 || x >= W || ground[y][x] === "p" || ground[y][x] === "b";
    for (let y = 0; y < H; y++)
      for (let x = 0; x < W; x++) {
        const px = x * T, py = y * T, k = ground[y][x], h = hash(x, y);
        q(px, py, T, T, (x + y) % 2 ? C.grass : C.grass2);
        if (k === "g") {
          if (h % 3 === 0) {
            q(px + (h % 11), py + (h % 7) + 4, 1, 2, C.tuft);
            q(px + (h % 11) + 2, py + (h % 7) + 3, 1, 3, C.tuft);
          }
          if (h % 13 === 0) {
            q(px + 5, py + 6, 2, 2, h % 2 ? C.flowerA : C.flowerB);
            q(px + 10, py + 11, 2, 2, C.flowerA);
          }
        } else if (k === "p") {
          q(px, py, T, T, C.path);
          if (h % 4 === 0) q(px + (h % 12) + 2, py + (h % 9) + 3, 2, 1, C.pebble);
          if (!isPath(x, y - 1)) q(px, py, T, 1, C.pathEdge);
          if (!isPath(x, y + 1)) q(px, py + T - 1, T, 1, C.pathEdge);
          if (!isPath(x - 1, y)) q(px, py, 1, T, C.pathEdge);
          if (!isPath(x + 1, y)) q(px + T - 1, py, 1, T, C.pathEdge);
        } else if (k === "w") {
          q(px, py, T, T, C.water);
          if (y === 18) q(px, py, T, 2, C.waterDeep);
        } else {
          q(px, py, T, T, C.wood);
          for (let i = 0; i < T; i += 4) q(px, py + i, T, 1, C.woodDk);
          q(x === 15 ? px : px + T - 2, py, 2, T, C.woodDk);
        }
      }
    if (has("pve")) {
      const x0 = PEN.x * T + 6, y0 = PEN.y * T + 8, x1 = (PEN.x + PEN.w - 1) * T + 10, y1 = (PEN.y + PEN.h - 1) * T + 10;
      q(x0 + 2, y0 + 2, x1 - x0 - 4, y1 - y0 - 4, "#8fb85a");
      for (const yy of [y0, y1]) {
        q(x0, yy, x1 - x0, 2, C.woodDk);
        q(x0, yy + 4, x1 - x0, 2, C.wood);
      }
      for (const xx of [x0, x1 - 2]) q(xx, y0, 2, y1 - y0 + 6, C.wood);
      for (let xx = x0; xx <= x1; xx += 8) {
        q(xx, y0 - 2, 2, 10, C.woodDk);
        q(xx, y1 - 2, 2, 10, C.woodDk);
      }
    }
    for (const [x, y] of trees) {
      const px = x * T, py = y * T;
      q(px + 6, py + 10, 4, 6, C.trunk);
      q(px + 2, py + 1, 12, 10, C.leaf);
      q(px + 1, py + 3, 14, 6, C.leaf);
      q(px + 4, py + 2, 4, 3, C.leafHi);
      q(px + 2, py + 9, 12, 2, C.leafDk);
    }
  }

  // ---------- buildings ----------
  function win(x: number, y: number, w: number, h: number, tone: Tone) {
    p(x - 1, y - 1, w + 2, h + 2, C.outline);
    if (tone === "quiet") {
      // shutters closed
      p(x, y, w, h, C.wood);
      p(x, y + Math.floor(h / 2), w, 1, C.woodDk);
      return;
    }
    p(x, y, w, h, C.winDk);
    lights.push([x, y, w, h]);
  }
  function roof(x: number, y: number, w: number, h: number, c: string, cd: string) {
    for (let i = 0; i < h; i++) {
      const inset = Math.max(0, Math.floor((h - i) / 2) - 2);
      p(x + inset, y + i, w - inset * 2, 1, i % 4 === 3 ? cd : c);
    }
    p(x, y + h, w, 2, cd);
  }
  type Draw = (px: number, py: number, pw: number, ph: number, s: Look, t: number) => void;
  const DRAW: Record<Building, Draw> = {
    pve(px, py, pw, ph, s) {
      roof(px - 2, py - 6, pw + 4, 30, C.roof, C.roofDk);
      p(px + 2, py + 26, pw - 4, ph - 26, C.barn);
      for (let x = px + 6; x < px + pw - 4; x += 6) p(x, py + 26, 1, ph - 26, C.barnDk);
      p(px + 2, py + 26, 2, ph - 26, C.trim);
      p(px + pw - 4, py + 26, 2, ph - 26, C.trim);
      p(px + 34, py + 4, 12, 10, C.trim);
      p(px + 36, py + 6, 8, 6, s.tone === "quiet" ? C.wood : C.outline);
      const dx = px + 32, dy = py + ph - 24;
      p(dx - 2, dy - 2, 20, 26, C.trim);
      p(dx, dy, 16, 24, C.barnDk);
      for (let i = 0; i < 16; i++) {
        p(dx + i, dy + Math.floor(i * 1.5), 1, 2, C.trim);
        p(dx + 15 - i, dy + Math.floor(i * 1.5), 1, 2, C.trim);
      }
      win(px + 10, py + 40, 9, 8, s.tone);
      win(px + pw - 19, py + 40, 9, 8, s.tone);
    },
    hist(px, py, pw, ph) {
      p(px + 66, py - 8, 6, 14, C.brick);
      p(px + 65, py - 9, 8, 2, C.outline);
      roof(px - 2, py - 4, pw + 4, 32, C.homieRoof, C.homieRoofDk);
      for (let i = 4; i < 32; i += 4) p(px + 6, py - 4 + i, 6, 1, C.homieRoofHi);
      p(px + 4, py + 30, pw - 8, ph - 30, C.wall);
      p(px + 4, py + ph - 3, pw - 8, 3, C.wallDk);
      p(px + 32, py + ph - 22, 16, 22, C.woodDk);
      p(px + 34, py + ph - 20, 12, 20, C.wood);
      p(px + 43, py + ph - 11, 2, 2, C.win);
      win(px + 12, py + 40, 12, 10, "ok");
      win(px + pw - 24, py + 40, 12, 10, "ok");
      if (has("hist")) {
        // the diary on its lectern
        p(px + pw + 2, py + ph - 12, 2, 12, C.woodDk);
        p(px + pw - 3, py + ph - 16, 12, 5, C.outline);
        p(px + pw - 2, py + ph - 15, 5, 3, C.trim);
        p(px + pw + 4, py + ph - 15, 4, 3, C.trim);
      }
    },
    k8s(px, py, pw, ph, s, t) {
      p(px, py + 6, pw, ph - 6, C.frame);
      for (let i = 0; i < 10; i++) p(px + 4 + i, (py + 6 - i * 0.6) | 0, pw - 8 - i * 2, 1, C.frame);
      p(px + 2, py + 8, pw - 4, ph - 10, C.glass);
      for (let x = px + 2; x < px + pw - 2; x += 12) p(x, py + 8, 1, ph - 10, C.frame);
      for (let i = 0; i < 18; i++) p(px + 6 + i, py + 10 + i, 2, 1, C.glassHi);
      const rows = Math.max(1, Math.min(5, s.n || 1));
      for (let r = 0; r < rows; r++) {
        const ry = py + 18 + r * 11;
        p(px + 4, ry + 6, pw - 8, 3, C.cropDead);
        for (let x = px + 6; x < px + pw - 8; x += 6) {
          const sick = s.tone === "bad" || (s.tone === "warn" && r === rows - 1);
          const c = s.tone === "quiet" ? C.cropDead : sick ? C.cropSick : C.crop;
          const sway = reduce ? 0 : Math.round(Math.sin(t / 600 + x) * 0.6);
          p(x + sway, ry, 3, 6, c);
          p(x + 1 + sway, ry - 1, 1, 2, sick ? C.cropDead : C.cropHi);
        }
      }
      p(px + 32, py + ph - 20, 16, 20, C.frame);
      p(px + 34, py + ph - 18, 12, 18, C.glassHi);
    },
    flux(px, py, pw, ph, s, t) {
      for (const lx of [px + 6, px + pw - 9]) p(lx, py + 30, 3, ph - 30, C.woodDk);
      p(px + 6, py + 50, pw - 12, 2, C.woodDk);
      p(px + 2, py - 6, pw - 4, 38, C.wood);
      for (let y = py; y < py + 32; y += 8) p(px + 2, y, pw - 4, 2, C.woodDk);
      p(px, py - 8, pw, 4, C.roofDk);
      const level = s.tone === "bad" ? 8 : s.tone === "quiet" ? 16 : 28;
      p(px + 10, py + 28 - level, 4, level, C.water2);
      if (has("k8s")) {
        // the pipe into the greenhouse
        p(px - 18, py + 30, 22, 4, C.metalDk);
        p(px - 18, py + 31, 22, 1, C.metal);
        if (s.tone === "ok" && !reduce)
          for (let i = 0; i < 3; i++) {
            const d = ((t / 25 + i * 8) % 22) | 0;
            p(px + 2 - d, py + 31, 2, 2, C.water2);
          }
        if (s.tone === "bad") p(px - 8, py + 34 + (((t / 120) % 10) | 0), 2, 3, C.water2);
      }
    },
    graf(px, py, pw, ph, s, t) {
      for (let i = 0; i < 26; i++) {
        const w = Math.min(pw + 6, 18 + i * 3);
        p(px + pw / 2 - w / 2, py + ph - 26 + i, w, 1, i < 4 ? C.hillHi : C.hill);
      }
      p(px + pw / 2 - 1, py + 4, 2, ph - 28, C.woodDk);
      const a = reduce ? 0 : Math.sin(t / 900) * 3;
      p(px + pw / 2 - 9 + a, py + 10, 18, 2, C.outline);
      p(px + pw / 2 + 7 + a, py + 8, 3, 6, C.outline);
      p(px + pw / 2 - 11 + a, py + 9, 4, 4, C.metal);
      p(px + pw / 2 - 3, py + 2, 6, 3, C.metal);
      if (s.tone === "bad" || s.tone === "warn") {
        // a cloud, raining while alerts fire
        const cx = px + pw / 2 - 14 + (reduce ? 0 : Math.sin(t / 1400) * 3);
        const cc = s.tone === "bad" ? "#5b6170" : "#a3a9b4";
        p(cx, py - 14, 28, 8, cc);
        p(cx + 4, py - 18, 14, 6, cc);
        p(cx + 14, py - 17, 10, 5, cc);
        if (s.tone === "bad")
          for (let i = 0; i < 6; i++) {
            const yy = reduce ? 4 : (t / 18 + i * 7) % 26;
            p(cx + 3 + i * 4, py - 6 + yy, 1, 3, C.water2);
          }
      }
    },
    bell(px, py, _pw, ph, s, t) {
      p(px + 7, py + 2, 2, ph - 2, C.woodDk);
      p(px + 1, py, 14, 3, C.roofDk);
      const swing = reduce || s.tone === "ok" ? 0 : Math.round(Math.sin(t / 120));
      p(px + 4 + swing, py + 4, 8, 7, C.gold);
      p(px + 3 + swing, py + 10, 10, 2, C.gold);
      p(px + 7 + swing, py + 12, 2, 2, C.woodDk);
    },
    gh(px, py, pw, ph, s) {
      roof(px - 2, py - 6, pw + 4, 26, C.roof, C.roofDk);
      p(px + 2, py + 22, pw - 4, ph - 22, "#9fb0bf");
      for (let y = py + 26; y < py + ph; y += 6) p(px + 2, y, pw - 4, 1, "#8494a3");
      p(px + 26, py + ph - 22, 28, 22, C.woodDk);
      for (let x = px + 28; x < px + 54; x += 4) p(x, py + ph - 20, 2, 20, C.wood);
      const gx = px + 34, gy = py + 4;
      p(gx, gy, 12, 12, C.win);
      p(gx + 4, gy - 2, 4, 16, C.win);
      p(gx - 2, gy + 4, 16, 4, C.win);
      p(gx + 4, gy + 4, 4, 4, C.roofDk);
      win(px + 8, py + 34, 10, 8, s.tone);
      win(px + pw - 18, py + 34, 10, 8, s.tone);
    },
    logs(px, py, pw, ph, s) {
      roof(px - 2, py - 6, pw + 4, 26, C.slate, C.slateDk);
      p(px + 2, py + 22, pw - 4, ph - 22, C.wall);
      p(px + 2, py + 22, pw - 4, 3, C.wallDk);
      for (const cx of [px + 6, px + pw - 10]) p(cx, py + 22, 4, ph - 22, C.trim);
      p(px + 32, py + ph - 22, 16, 22, C.slateDk);
      p(px + 34, py + ph - 20, 12, 20, "#4f86a8");
      p(px + 34, py + 4, 12, 9, C.trim);
      p(px + 35, py + 5, 5, 7, "#c0504d");
      p(px + 40, py + 5, 5, 7, "#4f86a8");
      win(px + 13, py + 32, 10, 12, s.tone);
      win(px + pw - 23, py + 32, 10, 12, s.tone);
    },
    well(px, py, pw, _ph, s) {
      p(px + 2, py + 2, 2, 14, C.woodDk);
      p(px + pw - 4, py + 2, 2, 14, C.woodDk);
      p(px, py, pw, 4, C.roofDk);
      p(px + 2, py + 14, pw - 4, 14, C.stoneDk);
      p(px + 4, py + 16, pw - 8, 10, C.stone);
      p(px + 6, py + 16, pw - 12, 4, s.tone === "bad" ? C.cropDead : C.water);
      p(px + pw / 2 - 1, py + 4, 2, 10, C.metalDk);
    },
    upd(px, py, pw, ph, s) {
      p(px + 1, py + 6, pw - 2, ph - 6, C.wood);
      for (let y = py + 10; y < py + ph; y += 5) p(px + 1, y, pw - 2, 1, C.woodDk);
      const cols = ["#e8743b", "#8fb85a", "#c9a0dc", "#f2c14e"];
      for (let i = 0; i < Math.min(4, s.n); i++) p(px + 4 + i * 6, py + 2 - (i % 2) * 2, 5, 6, cols[i]);
      p(px - 1, py + 4, pw + 2, 3, C.woodDk);
    },
    sec(px, py, pw, ph, s, t) {
      p(px + 2, py + 6, pw - 4, ph - 6, C.wood);
      p(px, py + 2, pw, 6, C.barn);
      p(px, py + 6, pw, 2, C.barnDk);
      win(px + 8, py + 18, pw - 16, 10, s.tone);
      p(px + 16, py + ph - 18, 14, 18, C.woodDk);
      // the dog's house, and the dog: ears up when something is found
      const hx = DOG.x * T, hy = DOG.y * T;
      roof(hx - 2, hy - 2, 30, 10, C.barn, C.barnDk);
      p(hx + 2, hy + 10, 24, 20, C.wood);
      p(hx + 9, hy + 18, 10, 12, C.outline);
      const wag = reduce ? 0 : Math.floor(t / 180) % 2, alert = s.tone === "warn" || s.tone === "bad" ? 1 : 0;
      const dx = hx + 6, dy = hy + 30;
      p(dx, dy, 12, 6, "#b07a45");
      p(dx + 9, dy - 4 - alert, 6, 6, "#b07a45");
      p(dx + 13, dy - 2, 1, 1, C.outline);
      p(dx + 9, dy - 6 - alert, 2, 3, "#8a5a33");
      p(dx - 2, dy - 1 - wag, 3, 2, "#b07a45");
      p(dx + 1, dy + 6, 2, 2, "#8a5a33");
      p(dx + 8, dy + 6, 2, 2, "#8a5a33");
    },
    cert(px, py, pw, ph, s) {
      p(px + 4, py + 10, 3, ph - 10, C.woodDk);
      p(px + pw - 7, py + 10, 3, ph - 10, C.woodDk);
      p(px + 1, py + 4, pw - 2, 26, C.woodDk);
      p(px + 3, py + 6, pw - 6, 22, C.wood);
      for (let i = 0; i < 5; i++) {
        const late = (s.tone === "warn" || s.tone === "bad") && i === 2;
        p(px + 6 + (i % 3) * 13, py + 8 + Math.floor(i / 3) * 10, 10, 8, late ? TONE[s.tone] : C.trim);
      }
      p(px + 1, py + 2, pw - 2, 3, C.roofDk);
    },
    bao(px, py, pw, ph, s) {
      p(px + 2, py + 4, pw - 4, ph - 4, C.stone);
      for (let y = py + 8; y < py + ph; y += 7) for (let x = px + 2 + ((y / 7) % 2) * 6; x < px + pw - 4; x += 12) p(x, y, 10, 1, C.stoneDk);
      p(px, py, pw, 6, C.stoneDk);
      const cx = px + pw / 2, cy = py + ph - 18;
      p(cx - 13, cy - 6, 26, 24, C.metalDk);
      p(cx - 11, cy - 4, 22, 22, s.tone === "bad" ? C.stoneDk : C.metal);
      p(cx - 2, cy + 4, 4, 4, C.metalDk);
      p(cx - 8, cy + 6, 16, 1, C.metalDk);
    },
    ts(px, py) {
      // the signpost at the bridge
      p(px - 10, py - 14, 2, 14, C.woodDk);
      p(px - 22, py - 14, 20, 7, C.wood);
      p(px - 20, py - 12, 16, 3, C.woodDk);
    },
  };

  function decorate(id: Building, px: number, py: number, pw: number, s: Look, t: number) {
    if (id === "ts") return;
    const fx = px + pw - 6, fy = py - 18;
    p(fx, fy, 1, 16, C.outline);
    const wave = reduce ? 0 : Math.floor(t / 250) % 2;
    p(fx + 1, fy, 8, 5, C.outline);
    p(fx + 1, fy + 1, 7, 3, TONE[s.tone]);
    p(fx + 4 + wave * 2, fy + 1 + wave, 2, 1, "rgba(0,0,0,.25)");
    if (s.tone === "warn") {
      const bob = reduce ? 0 : Math.round(Math.sin(t / 220) * 1.5);
      const bx = px + pw / 2 - 4, by = py - 24 + bob;
      p(bx, by, 9, 10, C.outline);
      p(bx + 1, by + 1, 7, 8, "#fff7e0");
      p(bx + 4, by + 2, 1, 4, C.outline);
      p(bx + 4, by + 7, 1, 1, C.outline);
    }
    if (s.tone === "bad" && !reduce && Math.random() < 0.08) puffs.push({ x: px + pw / 2 + (Math.random() - 0.5) * pw * 0.6, y: py - 2, life: 1 });
    if (s.tone === "quiet") {
      const z = reduce ? 0 : (t / 60) % 16;
      g.globalAlpha = 1 - z / 16;
      p(px + pw / 2 + 4 + z / 3, py - 10 - z, 4, 1, "#fff");
      p(px + pw / 2 + 6 + z / 3, py - 9 - z, 1, 1, "#fff");
      p(px + pw / 2 + 4 + z / 3, py - 8 - z, 4, 1, "#fff");
      g.globalAlpha = 1;
    }
  }

  // ---------- chickens: one per running guest, up to 6 ----------
  const chickens = Array.from({ length: 6 }, (_, i) => ({ x: 124 + i * 9, y: 60 + (i % 3) * 14, vx: 0, vy: 0, wait: i * 300 }));
  function drawChickens(n: number, dt: number) {
    for (const c of chickens.slice(0, Math.min(n, 6))) {
      if (!reduce) {
        c.wait -= dt;
        if (c.wait < 0) {
          const a = Math.random() * 6.28, m = Math.random() < 0.5 ? 0 : 0.02;
          c.vx = Math.cos(a) * m;
          c.vy = Math.sin(a) * m;
          c.wait = 600 + Math.random() * 1600;
        }
        c.x = Math.max(PEN.x * T + 14, Math.min((PEN.x + PEN.w - 1) * T - 2, c.x + c.vx * dt));
        c.y = Math.max(PEN.y * T + 18, Math.min((PEN.y + PEN.h - 1) * T, c.y + c.vy * dt));
      }
      const x = c.x | 0, y = c.y | 0, f = c.vx < 0;
      p(x, y + 6, 7, 1, "rgba(0,0,0,.2)");
      p(x, y + 1, 7, 5, "#fbf7ee");
      p(f ? x - 1 : x + 5, y - 1, 3, 3, "#fbf7ee");
      p(f ? x - 1 : x + 6, y - 2, 2, 1, "#e8743b");
      p(f ? x - 2 : x + 8, y, 1, 1, "#f2c14e");
      p(f ? x : x + 6, y, 1, 1, C.outline);
      p(x + 2, y + 6, 1, 1, "#f2c14e");
      p(x + 4, y + 6, 1, 1, "#f2c14e");
    }
  }

  // ---------- homie, input and paths ----------
  const START = { x: 15 * T + 8, y: 8 * T + 12 };
  const hero = { ...START, dir: "down" as Dir, frame: 0, anim: 0, path: null as [number, number][] | null, moving: false };
  const keys = new Set<Dir>();
  let active: Building | null = null;
  let hover: Building | null = null;
  const solidPx = (x: number, y: number) => {
    const tx = Math.floor(x / T), ty = Math.floor(y / T);
    return tx < 0 || ty < 0 || tx >= W || ty >= H || solid[ty][tx];
  };
  const canStand = (x: number, y: number) => !solidPx(x - 4, y - 3) && !solidPx(x + 3, y - 3) && !solidPx(x - 4, y) && !solidPx(x + 3, y);
  function bfs(sx: number, sy: number, gx: number, gy: number): [number, number][] | null {
    const prev = new Map<number, [number, number] | null>([[sy * W + sx, null]]);
    const queue: [number, number][] = [[sx, sy]];
    while (queue.length) {
      const [x, y] = queue.shift()!;
      if (x === gx && y === gy) break;
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
        const nx = x + dx, ny = y + dy;
        if (nx < 0 || ny < 0 || nx >= W || ny >= H || solid[ny][nx] || prev.has(ny * W + nx)) continue;
        prev.set(ny * W + nx, [x, y]);
        queue.push([nx, ny]);
      }
    }
    if (!prev.has(gy * W + gx)) return null;
    const out: [number, number][] = [];
    for (let c: [number, number] | null = [gx, gy]; c; c = prev.get(c[1] * W + c[0]) ?? null) out.unshift(c);
    return out.slice(1).map(([x, y]) => [x * T + 8, y * T + 12]);
  }
  function plotAt(tx: number, ty: number): Building | null {
    for (const id of built()) {
      const b = PLOTS[id];
      if (tx >= b.x && tx < b.x + b.w && ty >= b.y && ty < b.y + b.h) return id;
    }
    if (has("sec") && tx >= DOG.x && tx < DOG.x + DOG.w && ty >= DOG.y && ty < DOG.y + DOG.h) return "sec";
    return null;
  }
  function walkTo(tx: number, ty: number) {
    hero.path = bfs(Math.floor(hero.x / T), Math.floor((hero.y - 2) / T), tx, ty);
  }
  const toMap = (e: PointerEvent) => {
    const r = cv.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * MAP_W, ((e.clientY - r.top) / r.height) * MAP_H];
  };
  const onMove = (e: PointerEvent) => {
    const [x, y] = toMap(e);
    hover = plotAt(Math.floor(x / T), Math.floor(y / T));
    cv.style.cursor = hover ? "pointer" : "default";
  };
  const onLeave = () => (hover = null);
  const onDown = (e: PointerEvent) => {
    cv.focus({ preventScroll: true });
    const [x, y] = toMap(e), tx = Math.floor(x / T), ty = Math.floor(y / T), id = plotAt(tx, ty);
    if (id) walkTo(...PLOTS[id].door);
    else if (!solid[ty]?.[tx]) walkTo(tx, ty);
  };
  const MOVE: Record<string, Dir> = { ArrowUp: "up", KeyW: "up", ArrowDown: "down", KeyS: "down", ArrowLeft: "left", KeyA: "left", ArrowRight: "right", KeyD: "right" };
  const onKey = (e: KeyboardEvent) => {
    if (MOVE[e.code]) {
      keys.add(MOVE[e.code]);
      hero.path = null;
      e.preventDefault();
    } else if (["Enter", "NumpadEnter", "Space", "KeyE"].includes(e.code) && active) {
      on.open(active);
      e.preventDefault();
    } else if (e.code === "Escape") cv.blur();
  };
  const onKeyUp = (e: KeyboardEvent) => {
    if (MOVE[e.code]) keys.delete(MOVE[e.code]);
  };
  const onBlur = () => keys.clear();
  cv.addEventListener("pointermove", onMove);
  cv.addEventListener("pointerleave", onLeave);
  cv.addEventListener("pointerdown", onDown);
  cv.addEventListener("keydown", onKey);
  cv.addEventListener("keyup", onKeyUp);
  cv.addEventListener("blur", onBlur);

  function update(dt: number) {
    let dx = 0, dy = 0;
    if (keys.has("left")) dx--;
    if (keys.has("right")) dx++;
    if (keys.has("up")) dy--;
    if (keys.has("down")) dy++;
    if (hero.path?.length) {
      const [px, py] = hero.path[0], ddx = px - hero.x, ddy = py - hero.y, d = Math.hypot(ddx, ddy);
      if (d < 1.5) {
        hero.x = px;
        hero.y = py;
        hero.path.shift();
      } else {
        dx = ddx / d;
        dy = ddy / d;
      }
    }
    const len = Math.hypot(dx, dy);
    hero.moving = len > 0;
    if (hero.moving) {
      dx /= len;
      dy /= len;
      hero.dir = Math.abs(dx) > Math.abs(dy) ? (dx < 0 ? "left" : "right") : dy < 0 ? "up" : "down";
      const nx = hero.x + dx * 0.06 * dt, ny = hero.y + dy * 0.06 * dt;
      if (hero.path || canStand(nx, hero.y)) hero.x = nx;
      if (hero.path || canStand(hero.x, ny)) hero.y = ny;
      hero.anim += dt;
      hero.frame = Math.floor(hero.anim / 140) % 4;
    } else {
      hero.frame = 0;
      hero.anim = 0;
    }
    let near: Building | null = null;
    for (const id of built()) {
      const [tx, ty] = PLOTS[id].door;
      if (Math.hypot(tx * T + 8 - hero.x, ty * T + 12 - hero.y) < 14) near = id;
    }
    if (near !== active) {
      active = near;
      on.near(near);
    }
  }

  function timeOfDay() {
    if (quiet) return "night";
    const h = new Date().getHours();
    return h < 6 || h >= 20 ? "night" : h < 10 ? "morning" : h < 17 ? "day" : "dusk";
  }

  function draw(t: number, dt: number) {
    if (!bg) return;
    lights.length = 0;
    g.drawImage(bg, 0, 0);
    if (!reduce)
      for (let x = 0; x < MAP_W; x += 9) {
        const bridge = has("ts") && x >= 15 * T && x < 17 * T;
        if (Math.sin(t / 500 + x) > 0.6 && !bridge) p(x, 18 * T + 6 + ((x * 7) % 18), 4, 1, C.water2);
      }
    const anyBad = built().some((id) => looks[id]!.tone === "bad");
    const mood: Mood = quiet ? "sleep" : anyBad ? "sweat" : null;
    const items: { y: number; fn: () => void }[] = [];
    for (const id of built()) {
      const b = PLOTS[id], s = looks[id]!;
      const px = b.x * T, py = b.y * T, pw = b.w * T, ph = b.h * T;
      items.push({
        y: (b.y + b.h) * T,
        fn: () => {
          DRAW[id](px, py, pw, ph, s, t);
          decorate(id, px, py, pw, s, t);
          if (hover === id) {
            g.strokeStyle = "rgba(255,255,255,.85)";
            g.lineWidth = 1;
            g.strokeRect(px - 3.5, py - 9.5, pw + 7, ph + 12);
          }
        },
      });
    }
    if (!has("hist")) {
      const b = PLOTS.hist;
      items.push({ y: (b.y + b.h) * T, fn: () => DRAW.hist(b.x * T, b.y * T, b.w * T, b.h * T, { tone: "ok", n: 0 }, t) });
    }
    if (has("pve")) items.push({ y: PEN.y * T + 40, fn: () => drawChickens(looks.pve!.n, dt) });
    items.push({ y: hero.y, fn: () => drawHomie(g, Math.round(hero.x) - 8, Math.round(hero.y) - 20, hero.dir, hero.frame, hero.moving ? (anyBad ? "sweat" : null) : mood) });
    items.sort((a, b) => a.y - b.y).forEach((i) => i.fn());
    for (const f of puffs) {
      f.y -= dt * 0.012;
      f.x += Math.sin(f.y / 5) * 0.2;
      f.life -= dt / 2200;
      g.globalAlpha = Math.max(0, f.life) * 0.8;
      const r = 3 + (1 - f.life) * 4;
      p(f.x - r / 2, f.y - r / 2, r, r, "#4b4f58");
    }
    g.globalAlpha = 1;
    for (let i = puffs.length - 1; i >= 0; i--) if (puffs[i].life <= 0) puffs.splice(i, 1);
    const tod = timeOfDay();
    const tint = { morning: "rgba(255,170,90,.12)", day: null, dusk: "rgba(255,110,60,.22)", night: "rgba(14,18,58,.55)" }[tod];
    if (tint) p(0, 0, MAP_W, MAP_H, tint);
    if (tod === "night" || tod === "dusk") {
      for (const [x, y, w, h] of lights) {
        g.globalAlpha = 0.25;
        p(x - 3, y - 3, w + 6, h + 6, C.win);
        g.globalAlpha = 1;
        p(x, y, w, h, C.win);
      }
      if (tod === "night") {
        p(470, 6, 10, 10, "#f4efd8");
        p(473, 6, 7, 7, "rgba(14,18,58,.9)");
      }
    }
  }

  let last = performance.now(), acc = 0, raf = 0;
  const loop = (now: number) => {
    raf = requestAnimationFrame(loop);
    const dt = Math.min(100, now - last);
    acc += dt;
    last = now;
    if (!bg) acc = 0;
    if (acc < 33) return; // ponytail: a 30 fps cap is plenty for 16 px tiles
    update(acc);
    draw(now, acc);
    acc = 0;
  };
  const onVisible = () => {
    cancelAnimationFrame(raf);
    if (!document.hidden) {
      last = performance.now();
      raf = requestAnimationFrame(loop);
    }
  };
  document.addEventListener("visibilitychange", onVisible);
  raf = requestAnimationFrame(loop);

  let shape = "";
  return {
    set(next, q) {
      looks = next;
      quiet = q;
      const s = built().join(",");
      if (s !== shape) {
        shape = s;
        rebuild();
        if (!canStand(hero.x, hero.y)) Object.assign(hero, START, { path: null });
      }
    },
    stop() {
      cancelAnimationFrame(raf);
      document.removeEventListener("visibilitychange", onVisible);
      cv.removeEventListener("pointermove", onMove);
      cv.removeEventListener("pointerleave", onLeave);
      cv.removeEventListener("pointerdown", onDown);
      cv.removeEventListener("keydown", onKey);
      cv.removeEventListener("keyup", onKeyUp);
      cv.removeEventListener("blur", onBlur);
    },
  };
}
