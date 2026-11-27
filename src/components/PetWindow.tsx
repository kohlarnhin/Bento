import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emitTo, type EventCallback } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import './pet.css';
import { readPastimesOff } from '../lib/petPastimes';

type Tone = 'error' | 'notify' | 'done' | 'working' | 'running';
type Item = { id: string; projectId: string | null; slot: number | null; text: string; tone: Tone; urgent: boolean };
type View = { mood: 'idle' | 'starting' | 'error' | 'notify'; running: number; thinking?: number; errors: number; doneAt: number | null; quiet: boolean; items: Item[] };
type Dock = 'top' | 'left' | 'right' | 'bottom' | 'inside' | 'corner';
type Place = { dock: Dock; active: boolean; ledge: number };
// Two pets share this component: one on Bento's top edge, one in the corner. Only one is shown at a time.
const ON_BENTO = getCurrentWindow().label === 'bento-pet-dock';
// Only events sent to THIS window. The global listen() also hears what pet.rs sends to the other pet
// window, so the pet on Bento picked up the corner pet's {dock: 'corner'} and turned into the corner pet.
const THIS_WINDOW = getCurrentWebviewWindow();
const listen = <T,>(event: string, handler: EventCallback<T>) => THIS_WINDOW.listen<T>(event, handler);
type Pose = 'perch' | 'sit' | 'prone' | 'peek' | 'hang'
  | 'mug' | 'mug-sip' | 'type-a' | 'type-l' | 'type-r' | 'book' | 'book-flip' | 'water' | 'water-pour' | 'sword' | 'sword-raise' | 'sword-cut'
  | 'stand' | 'stand-laptop' | 'stand-mug' | 'stand-book' | 'stand-can' | 'stand-hat'
  | 'sword-salute' | 'sword-spin'
  | 'walk' | 'walk-hp' | 'edge-sit' | 'edge-kick-l' | 'edge-kick-r' | 'music' | 'wave-a' | 'wave-b' | 'reach'
  | 'scarf-hold' | 'scarf-tug' | 'scarf-catch'
  | 'stretch-up' | 'cube-a' | 'cube-b' | 'doze' | 'bubble-a' | 'bubble-b' | 'plane-a' | 'plane-b' | 'snack-a' | 'snack-b';
type Inspection = { path: string; exists: boolean; isDir: boolean; name: string; existingProject: { id: string; name: string } | null };
type Style = { opacity: number };
type Face = 'idle' | 'happy' | 'error' | 'alert' | 'working' | 'sleep' | 'curious' | 'yawn' | 'typing' | 'squint'
  | 'content' | 'cool' | 'surprised' | 'shy' | 'love' | 'dizzy' | 'pout' | 'think' | 'angry' | 'smug' | 'music';
type Motion = 'none' | 'walk' | 'run' | 'hop' | 'sway' | 'stretch' | 'peek' | 'look' | 'nod' | 'bob' | 'wiggle' | 'tilt'
  | 'kick' | 'tap' | 'sneeze' | 'swing' | 'duck' | 'lean' | 'squat' | 'jump' | 'bend' | 'sip' | 'pour' | 'breathe' | 'slash' | 'dangle' | 'pat' | 'beat' | 'still';
type Bubble = { key: string; text: string; tone: Tone | 'tip'; item?: Item };

// Window layout in logical px; mirrored by the constants at the top of src-tauri/src/pet.rs.
const WIN_W = 320;
const WIN_H_PX = 220;
const LEDGE = 150;
const WALL = 20;
const HOME_X = 240;
// On the top edge the window spans Bento's width plus DOCK_PAD on each side (pet.rs); the character
// walks inside it, EDGE_MARGIN in from Bento's corners. Its spot along the edge is 0..1.
const DOCK_PAD = 200;
const EDGE_MARGIN = 56;
// Corner pet: where its right side sits in the window (pet.rs places the window to match).
// Where the corner pet's face sits across its window.
const CORNER_X = 232;
/**
 * The top-edge walk is a sprite sheet: 19 frames of one continuous walk cycle (round 9), cut from an
 * image-to-video clip of Bento walking (16 fps, matted, scaled to c-walk-0's height, feet on the same
 * ground line). One strip per variant (c-walk-sheet.webp, c-walk-hp-sheet.webp), 240x333 per cell,
 * both starting at the same leg phase. Hard cuts; the column follows ground covered, so pace and feet match.
 */
const WALK_CYCLE = Array.from({ length: 19 }, (_, i) => i);
const WALK_POSES = WALK_CYCLE.length;
/** Ground one full cycle (two steps) covers at the top-edge scale, in logical px: ~188 sheet px per cycle x 0.28 (120/240 x 0.56). */
const WALK_CYCLE_PX = 52.6;
/** Walking pace in px/s: ~1.23 s per cycle, ~15 fps, close to the video's 16 fps. */
const WALK_SPEED = 42.6;
/** Standing still it shows the first pose, and a walk starts from there. */
const WALK_STAND = 0;
const walkSheet = (pose: 'walk' | 'walk-hp') => `/pet/c-${pose}-sheet.webp`;
// Decode both sheets up front so the first step never shows a blank frame.
const walkSheets = typeof Image === 'undefined' ? [] : (['walk', 'walk-hp'] as const).map(pose => {
  const image = new Image();
  image.src = walkSheet(pose);
  void image.decode?.().catch(() => undefined);
  return image;
});
void walkSheets;
/** Centre of the raised fist on `sword-spin`, in image units: the jian's grip sits in it. */
const FIST: [number, number] = [13.0, 76.4];
// Peeking from a side / hanging from the bottom: where the character sits in its window.
const SIDE_Y = 18;
const HANG_Y = 10;
// On Bento it is small, like a figure sitting on the window; in the corner a bit bigger.
const SCALE: Record<Dock, number> = { top: 0.56, left: 0.56, right: 0.56, bottom: 0.56, inside: 0.56, corner: 0.8 };
const STYLE_KEY = 'bento-pet-style';
const AFFECTION_KEY = 'bento-pet-affection';
// On the stool frames the body breathes on its own: everything above this line (fraction of the
// image height, where the seat meets the stool) scales; the stool and feet stay put.
const SEAT = 0.735;
// Winter (mid-November to February): the stool frames swap to their scarf versions (c-*-w.webp); snow from December.
const season = () => {
  const now = new Date();
  const month = now.getMonth();
  return { scarf: month === 11 || month <= 1 || (month === 10 && now.getDate() >= 15), snow: month === 11 || month <= 1 };
};

// Each pose image (public/pet/<pose>.webp, rendered at 3x): its size, the line it rests
// on, and the dark visor where the face is drawn ([x, y, width, height], tilt in degrees).
// Redesigned 2026-11-24: one neckless bean-shaped plush (cream fleece, big oval visor with a thin metal rim,
// dark headband and grey ear pods with coral rings, grey mittens). The visors are blank glass; PetFace draws the face.
// Seated poses rest their seat on the line (legs hanging over it); standing poses rest their feet on it.
// The corner's seated poses sit on a little round wooden stool, so they rest the stool's feet on the line.
// Their shins (and the stool under them) were shortened to match the standing legs: the art keeps its size, with
// the feet now 12.9 units above the image's bottom (ledge 145.4 of 158.3). The edge-sit frames' hanging legs likewise.
const IMAGES: Record<string, { w: number; h: number; ledge: number; screen: [number, number, number, number]; faceBox?: [number, number, number, number]; tilt: number; inset: number; src?: string; face?: false }> = {
  'sit': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-sit' },
  'mug': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.5], tilt: 0, inset: 0.1, src: 'c-mug' },
  'mug-sip': { w: 148.1, h: 158.3, ledge: 145.4, screen: [39.0, 19.3, 57.6, 44.4], tilt: 0, inset: 0.1, src: 'c-mug-sip' },
  'tap-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.3, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-tap-a' },
  // Typing: a full-size laptop on the lap, mittens down on its keys behind the lid. The keystroke frames
  // share that image; the body leans a touch toward the typing hand (pet.css).
  'tap-l': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.3, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-tap-a' },
  'tap-r': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.3, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-tap-a' },
  // Reading: an open book held in both mittens, both halves lying open (c-book-flat). Turning a page is
  // drawn over it (PageTurn): the sheet rises from the right half, stands up over the spine and lands on the left.
  'book': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.3, 19.0, 65.1, 54.2], tilt: 0, inset: 0.1, src: 'c-book-flat' },
  'book-flip': { w: 148.1, h: 158.3, ledge: 145.4, screen: [42.0, 19.0, 64.7, 53.5], tilt: 0, inset: 0.1, src: 'c-book-flip' },
  // Watering: on the same stool, a sage can in both mittens over its own pot on the ground (water-pour tips it).
  'water': { w: 148.1, h: 158.3, ledge: 145.4, screen: [42.0, 19.3, 65.1, 53.5], tilt: 0, inset: 0.1, src: 'c-water-crouch' },
  'water-pour': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.4, 53.9], tilt: 0, inset: 0.1, src: 'c-water-pour' },
  // Wuxia swordsman: a woven bamboo douli and a jian with a red tassel, standing.
  // Ready pose, stance, wind-up and finishing slash.
  'sword-salute': { w: 131.0, h: 156.0, ledge: 156.0, screen: [30.3, 33.0, 59.0, 49.3], tilt: 0, inset: 0.1, src: 'c-sword-salute' },
  'sword': { w: 134.0, h: 152.0, ledge: 152.0, screen: [32.7, 37.3, 54.7, 47.7], tilt: 0, inset: 0.1, src: 'c-sword' },
  'sword-raise': { w: 132.0, h: 163.3, ledge: 163.3, screen: [22.0, 46.0, 55.7, 48.7], tilt: 0, inset: 0.1, src: 'c-sword-raise' },
  'sword-cut': { w: 129.7, h: 145.7, ledge: 145.7, screen: [29.7, 32.7, 55.7, 47.7], tilt: 0, inset: 0.1, src: 'c-sword-cut' },
  // The flourish (挽剑花): body and hat hold still while the jian (c-blade) draws a figure eight round the raised fist.
  'sword-spin': { w: 128.0, h: 151.0, ledge: 151.0, screen: [34.0, 33.0, 57.3, 48.0], tilt: 0, inset: 0.1, src: 'c-sword-spin' },
  // Top edge: an 8-pose walk cycle (side view facing right; mirrored when walking left), each pose held for two
  // of the 16 frames. The face sits on the edge-on visor (faceBox). walk-hp: the near mitten holds the ear pod.
  'walk-0': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.3, 42.8, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-0' },
  'walk-1': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.3, 42.8, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-1' },
  'walk-2': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [77.0, 44.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-2' },
  'walk-3': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [77.0, 44.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-3' },
  'walk-4': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [77.0, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-4' },
  'walk-5': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [77.0, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-5' },
  'walk-6': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 42.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-6' },
  'walk-7': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 42.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-7' },
  'walk-8': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.0, 42.7, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-8' },
  'walk-9': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.0, 42.7, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-9' },
  'walk-10': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 44.5, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-10' },
  'walk-11': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 44.5, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-11' },
  'walk-12': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 43.5, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-12' },
  'walk-13': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 43.5, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-13' },
  'walk-14': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.3, 42.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-14' },
  'walk-15': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.3, 42.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-15' },
  'walk-hp-0': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 43.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-0' },
  'walk-hp-1': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 43.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-1' },
  'walk-hp-2': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.9, 44.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-2' },
  'walk-hp-3': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.9, 44.3, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-3' },
  'walk-hp-4': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.9, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-4' },
  'walk-hp-5': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.9, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-5' },
  'walk-hp-6': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-6' },
  'walk-hp-7': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.7, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-7' },
  'walk-hp-8': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.5, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-8' },
  'walk-hp-9': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.5, 43.0, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-9' },
  'walk-hp-10': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 44.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-10' },
  'walk-hp-11': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 44.4, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-11' },
  'walk-hp-12': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 42.8, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-12' },
  'walk-hp-13': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.2, 42.8, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-13' },
  'walk-hp-14': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.0, 42.9, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-14' },
  'walk-hp-15': { w: 120.0, h: 166.7, ledge: 166.7, screen: [50, 40, 20, 50], faceBox: [76.0, 42.9, 22.0, 20.0], tilt: 0, inset: 0.06, src: 'c-walk-hp-15' },
  // Sitting ON the window's top border: seat on the edge line, legs hanging over it; edge-kick-l / -r swing one leg.
  'edge-sit': { w: 135.4, h: 151.7, ledge: 123.0, screen: [34.9, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'c-edge-a' },
  'edge-kick-l': { w: 135.4, h: 151.7, ledge: 123.0, screen: [34.9, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'c-edge-b' },
  'edge-kick-r': { w: 135.4, h: 151.7, ledge: 123.0, screen: [35.3, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'c-edge-c' },
  // Corner pastime: a mitten on the ear pod, bobbing to the beat.
  'music': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.5], tilt: 0, inset: 0.1, src: 'c-music' },
  // Saying hi: one mitten up, waving.
  'wave-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [42.0, 19.0, 64.7, 53.5], tilt: 0, inset: 0.1, src: 'c-wave-a' },
  'wave-b': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.3, 19.0, 65.1, 54.2], tilt: 0, inset: 0.1, src: 'c-wave-b' },
  // In-between: hands lifted, about to take (or just put away) whatever a pastime uses. Also how it catches a dropped folder.
  'reach': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.0, 19.0, 65.7, 53.9], tilt: 0, inset: 0.1, src: 'c-reach' },
  // Winter only: the scarf pastime. Holds the scarf out, wraps it on and tugs it snug, then catches a snowflake.
  'scarf-hold': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-scarf-1' },
  'scarf-tug': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-scarf-2' },
  // Idle pastimes (base art, no scarf): stretch, Rubik's cube, doze, bubbles, paper plane, onigiri.
  'stretch-up': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-stretch' },
  'cube-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-cube-a' },
  'cube-b': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-cube-b' },
  'doze': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-doze' },
  'bubble-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-bubble-a' },
  'bubble-b': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-bubble-b' },
  'plane-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-plane-a' },
  'plane-b': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-plane-b' },
  'snack-a': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-snack-a' },
  'snack-b': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-snack-b' },
  'scarf-catch': { w: 148.1, h: 158.3, ledge: 145.4, screen: [41.7, 19.0, 65.1, 53.9], tilt: 0, inset: 0.1, src: 'c-scarf-3' },
  // On the top edge these sit ON the border (same body and scale as edge-sit).
  'edge-music': { w: 135.4, h: 151.7, ledge: 123.0, screen: [35.3, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'c-edge-music' },
  'edge-wave-a': { w: 135.4, h: 151.7, ledge: 123.0, screen: [35.3, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'edge-src-c-wave-a' },
  'edge-wave-b': { w: 135.4, h: 151.7, ledge: 123.0, screen: [34.9, 19.0, 69.5, 56.7], tilt: 0, inset: 0.1, src: 'edge-src-c-wave-b' },
  prone: { w: 204.7, h: 133.0, ledge: 85.7, screen: [21.1, 26.3, 53.3, 44.7], tilt: -12, inset: 0.14 },
  peek: { w: 81.3, h: 135.0, ledge: 0, screen: [0.0, 19.4, 62.0, 50.6], tilt: 40, inset: 0.14 },
  hang: { w: 115.0, h: 143.7, ledge: 0, screen: [30.7, 14.7, 62.0, 50.7], tilt: 0, inset: 0.14 },
};
// Corner frames drawn on the stool: their effect offsets (steam, code, notes) are rescaled to the larger image.
const STOOL = new Set(['sit', 'mug', 'mug-sip', 'tap-a', 'tap-l', 'tap-r', 'book', 'book-flip', 'music', 'wave-a', 'wave-b', 'reach', 'scarf-hold', 'scarf-tug', 'scarf-catch',
  'stretch-up', 'cube-a', 'cube-b', 'doze', 'bubble-a', 'bubble-b', 'plane-a', 'plane-b', 'snack-a', 'snack-b']);
// The scarf is no longer a costume over every pose: it is its own winter pastime (see 'scarf' below).
if (season().scarf) for (const name of ['scarf-hold', 'scarf-tug', 'scarf-catch']) { const preload = new Image(); preload.src = `/pet/${IMAGES[name]?.src ?? name}.webp`; }
// Pose names the behaviour uses → the image that shows it. Entrances start from the plain seated pose.
const IMAGE_OF: Partial<Record<Pose, string>> = {
  perch: 'sit', stand: 'sit', 'stand-laptop': 'sit', 'stand-mug': 'sit', 'stand-book': 'sit', 'stand-can': 'sit',
  'stand-hat': 'sword-salute', 'type-a': 'tap-a', 'type-l': 'tap-l', 'type-r': 'tap-r',
};
const imageOf = (pose: Pose) => IMAGE_OF[pose] ?? pose;

// Decode every pose up front so switching never shows a blank frame.
for (const [name, image] of Object.entries(IMAGES)) { const preload = new Image(); preload.src = `/pet/${image.src ?? name}.webp`; }

const TIPS = [
  '⌘P 可以快速切换项目',
  '⌘⇧U 直接跳到需要你处理的终端',
  '终端里 ⌘⇧↩ 进入专注模式',
  '⌘T 给当前项目开一个新终端',
  '终端里 ⌘D 左右分屏',
  '右键我，可以快速启动或停止项目',
  '右键我，可以打开快捷菜单',
  '在角落戳戳我，或者摸摸我的头',
  '把项目文件夹拖到角落的我身上，我帮你在 Bento 里打开',
  '长按我可以把我拖到别处',
  '在角落双击我，就能把 Bento 叫回来',
];

// Corner pastimes and top-edge workouts, each played in shuffled rounds.
const SNOWFLAKE_MS = 1700;
const PLANE_MS = 2600;
const PASTIMES = ['drink', 'code', 'read', 'water', 'sword', 'music', 'stretch', 'cube', 'doze', 'bubbles', 'plane', 'snack'];
// Seasonal pastimes join the round only in their season.
// Settings → 桌宠 can take pastimes out of the round; if every one is off it falls back to all of them.
const pastimesNow = () => {
  const all = season().scarf ? [...PASTIMES, 'scarf'] : PASTIMES;
  const off = readPastimesOff();
  const on = all.filter(item => !off.includes(item));
  return on.length ? on : all;
};
// On Bento's top edge it just wanders back and forth, sometimes with headphones, and sits on the edge when tired.
const WORKOUTS = ['wander'];
const rounds = new Map<string, string[]>();
/** Whether the corner pet rests in the seated pose between pastimes. Off since round 10: pastimes cut straight to the next. */
const CORNER_REST = false;
const nextInRound = (list: string[], last: string | undefined): string => {
  const key = list.join();
  let round = rounds.get(key) ?? [];
  if (round.length === 0) {
    round = [...list].sort(() => Math.random() - 0.5);
    // Never the same thing twice in a row across rounds.
    if (round.length > 1 && round[0] === last) round.push(round.shift()!);
  }
  const next = round.shift()!;
  rounds.set(key, round);
  return next;
};

const ANNOUNCE_MS = 5200;
// Break reminder: after this long on Bento without a real break, it suggests one.
const BREAK_AFTER_MS = 60 * 60_000;
// Being away from Bento at least this long counts as a break and resets the clock.
const BREAK_RESET_MS = 5 * 60_000;
const BREAK_SHOW_MS = 15_000;
const SLEEP_AFTER_MS = 10 * 60_000;
const random = (min: number, max: number) => min + Math.random() * (max - min);
const pick = <T,>(weighted: [T, number][]): T => {
  const total = weighted.reduce((sum, [, weight]) => sum + Math.max(0, weight), 0);
  let roll = Math.random() * total;
  for (const [value, weight] of weighted) {
    roll -= Math.max(0, weight);
    if (roll <= 0) return value;
  }
  return weighted[0][0];
};
// Which image a pose shows right now: the walk alternates two strides; on the top
// edge any sitting is sitting ON the border with the legs hanging down.
const frameOf = (pose: Pose, stride: number, dock: Dock): string => {
  // Face/geometry metadata still comes from the 8 legacy walk entries (walk-0, -2, ... -14): map the column onto them.
  if (pose === 'walk' || pose === 'walk-hp') return `${pose}-${(Math.round(WALK_CYCLE[stride % WALK_POSES] * 8 / WALK_POSES) % 8) * 2}`;
  if (dock === 'top' && (pose === 'sit' || pose === 'perch')) return 'edge-sit';
  if (dock === 'top' && (pose === 'music' || pose === 'wave-a' || pose === 'wave-b')) return `edge-${pose}`;
  return imageOf(pose);
};
// Pose families: a swap inside one (tap-l → tap-r, book → book-flip) is part of the action and shows at once;
// a swap between two gets a short settle so poses blend instead of jumping.
const groupOf = (frame: string) => frame.startsWith('walk') ? 'walk' : frame.startsWith('tap-') ? 'tap' : frame.startsWith('edge-') ? 'edge'
  : frame.startsWith('sword') ? 'sword' : frame.replace(/-(a|b|flip|sip|pour)$/, '');
const reducedMotion = () => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
const lateNight = () => { const hour = new Date().getHours(); return hour >= 23 || hour < 6; };

export function PetWindow() {
  const [view, setView] = useState<View>();
  const [dock, setDock] = useState<Dock>(ON_BENTO ? 'top' : 'corner');
  const [active, setActive] = useState(false);
  const [ledge, setLedge] = useState(LEDGE);
  const [travel, setTravel] = useState<{ ms: number; dir: 1 | -1 }>();
  const [landing, setLanding] = useState(false);
  // Moving somewhere else on Bento: fades out here, appears there.
  const [away, setAway] = useState(false);
  const [pose, setPose] = useState<Pose>('sit');
  const [motion, setMotion] = useState<Motion>('none');
  const [walkMs, setWalkMs] = useState(0);
  const [walkDir, setWalkDir] = useState<1 | -1>(-1);
  // Walking: which column of the walk sheet shows (index into WALK_CYCLE).
  const [stride, setStride] = useState(WALK_STAND);
  // Saying hi: the wave frame shown over whatever it is doing.
  const [greet, setGreet] = useState<Pose>();
  const [restFace, setRestFace] = useState<Face>('idle');
  const [reaction, setReaction] = useState<Face>();
  const [hover, setHover] = useState(false);
  // Hover is read from the pointer position Rust sends (pet-gaze), not from pointerleave: once the
  // window lets clicks through again it gets no more mouse events, and a missed leave kept the
  // 「N 个项目运行中」 bubble up for good. 打开 / 稍后 on a hover bubble mute it until the pointer leaves.
  const hoverNow = useRef(false);
  const hoverMuted = useRef(false);
  const setHovering = (next: boolean) => { if (hoverNow.current !== next) { hoverNow.current = next; setHover(next); } };
  const muteHover = () => { hoverMuted.current = true; setHovering(false); };
  const [blink, setBlink] = useState(false);
  const [gaze, setGaze] = useState<[number, number]>([0, 0]);
  const [autoGaze, setAutoGaze] = useState<[number, number]>();
  const [bubble, setBubble] = useState<Bubble>();
  // A little burst of stars when something finishes (its start time, as a key).
  const [sparkle, setSparkle] = useState<number>();
  useEffect(() => {
    if (!sparkle) return;
    const timer = window.setTimeout(() => setSparkle(undefined), 1600);
    return () => window.clearTimeout(timer);
  }, [sparkle]);
  // Extras some actions draw: a fishing line, a sneeze puff.
  const [prop, setProp] = useState<'fish' | 'puff' | 'hearts' | 'slash' | 'steam' | 'sigh' | 'code' | 'sing' | 'pour' | 'petals' | 'blade' | 'flake' | 'zz' | 'bubbles' | 'plane'>();
  const [calmSince, setCalmSince] = useState(() => Date.now());
  // Reading: a page is being turned (its start time, as a key).
  const [pageTurn, setPageTurn] = useState<number>();
  // Corner only: resting in its plain seated pose between pastimes. Pokes and pats work only then;
  // a click during a pastime brings it back to this pose (resetKey restarts the behaviour loop).
  const [atBase, setAtBase] = useState(false);
  const atBaseRef = useRef(false);
  atBaseRef.current = atBase;
  const [resetKey, setResetKey] = useState(0);
  const pokedAt = useRef(0);
  // Opacity from the menu; remembered here, reported to pet.rs on start.
  const [style, setStyle] = useState<Style>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(STYLE_KEY) ?? 'null') as Partial<Style> | null;
      if (typeof saved?.opacity === 'number') return { opacity: saved.opacity };
    } catch { /* default */ }
    return { opacity: 100 };
  });
  // Corner only: another app is fullscreen or a meeting is in front, so it steps aside.
  const [tucked, setTucked] = useState(false);
  // A folder is being dragged over it: hands up to catch it.
  const [catching, setCatching] = useState(false);
  // Affection grows with pokes and pats and fades a little each day without them. Never shown as a number.
  const affection = useRef<number>((() => {
    try {
      const saved = JSON.parse(localStorage.getItem(AFFECTION_KEY) ?? 'null') as { value: number; at: number } | null;
      if (!saved) return 20;
      return Math.max(0, saved.value - Math.floor((Date.now() - saved.at) / 86_400_000) * 5);
    } catch { return 20; }
  })());
  const fondle = (amount: number) => {
    affection.current = Math.min(100, affection.current + amount);
    try { localStorage.setItem(AFFECTION_KEY, JSON.stringify({ value: affection.current, at: Date.now() })); } catch { /* ignore */ }
  };
  // Waiting items put off with 稍后, until when.
  const [snoozed, setSnoozed] = useState<Record<string, number>>({});
  // The visor ring flashes when something finishes.
  const [doneFlash, setDoneFlash] = useState(false);
  const pointer = useRef<[number, number]>([-999, -999]);
  const pointerMovedAt = useRef(0);
  const bodyRef = useRef<HTMLDivElement>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);
  const press = useRef<{ x: number; y: number; dragged: boolean }>();
  const clickTimer = useRef<number>();

  // ---- data ---------------------------------------------------------------
  const load = useCallback(async () => setView(await invoke<View>('get_pet_view')), []);
  useEffect(() => {
    let timer: number | undefined;
    const refresh = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => { void load().catch(() => undefined); }, 150);
    };
    const unRefresh = listen('pet-refresh', refresh);
    const unDock = listen<Place>('pet-place', ({ payload }) => { setDock(payload.dock); setActive(payload.active); setLedge(payload.ledge); });
    const unStyle = listen<Style>('pet-style', ({ payload }) => {
      setStyle(payload);
      try { localStorage.setItem(STYLE_KEY, JSON.stringify(payload)); } catch { /* ignore */ }
    });
    const unTuck = listen<boolean>('pet-tuck', ({ payload }) => setTucked(payload));
    void invoke<boolean>('pet_tucked').then(setTucked).catch(() => undefined);
    let travelTimer: number | undefined;
    const unTravel = listen<{ ms: number; dir: 1 | -1 }>('pet-travel', ({ payload }) => {
      window.clearTimeout(travelTimer);
      setTravel(payload);
      travelTimer = window.setTimeout(() => setTravel(undefined), payload.ms + 80);
    });
    const unGaze = listen<[number, number]>('pet-gaze', ({ payload }) => {
      const [x, y] = pointer.current;
      if (Math.abs(x - payload[0]) + Math.abs(y - payload[1]) > 6) pointerMovedAt.current = Date.now();
      pointer.current = payload;
      setGaze(payload);
      const [px, py] = payload;
      const within = (element: HTMLElement | null) => {
        const box = element?.getBoundingClientRect();
        return !!box && box.width > 0 && px >= box.left - 4 && px <= box.right + 4 && py >= box.top - 4 && py <= box.bottom + 4;
      };
      const onBody = within(bodyRef.current);
      const onBubble = !!bubbleRef.current?.dataset.visible && within(bubbleRef.current);
      if (!onBody && !onBubble) hoverMuted.current = false;
      // Moving from the body onto its bubble keeps the hover, so its buttons can be reached.
      setHovering(!hoverMuted.current && (onBody || (onBubble && hoverNow.current)));
    });
    void invoke<Place>('pet_dock').then(place => { setDock(place.dock); setActive(place.active); setLedge(place.ledge); }).catch(() => undefined);
    void unRefresh.then(refresh);
    const slow = window.setInterval(refresh, 30_000);
    return () => {
      window.clearTimeout(timer);
      window.clearInterval(slow);
      window.clearTimeout(travelTimer);
      for (const stop of [unRefresh, unDock, unTravel, unGaze, unStyle, unTuck]) void stop.then(unlisten => unlisten());
    };
  }, [load]);

  // pet.rs keeps the opacity for the menu's checkmarks; this window remembers it.
  useEffect(() => { void invoke('pet_style_init', { opacity: style.opacity }).catch(() => undefined); }, []);

  // A short bounce when the pet arrives somewhere new.
  const firstDock = useRef(true);
  useEffect(() => {
    if (firstDock.current) { firstDock.current = false; return; }
    setLanding(true);
    const timer = window.setTimeout(() => setLanding(false), 450);
    return () => window.clearTimeout(timer);
  }, [dock]);

  // Each time it is shown again (Bento came forward, or Bento was left) it says hi.
  const wasActive = useRef(false);
  // Until when it is saying hi; it stands still for that, then sets off.
  const greetUntil = useRef(0);
  useEffect(() => {
    const appeared = active && !wasActive.current;
    wasActive.current = active;
    if (!appeared) return;
    greetUntil.current = Date.now() + 8 * 220 + 200;
    // Faces us and waves: the paw swings back and forth a few times (with a little sparkle once it is fond of us).
    setReaction('happy');
    if (affection.current >= 60) setSparkle(Date.now());
    let waves = 0;
    setGreet('wave-a');
    const timer = window.setInterval(() => {
      waves += 1;
      if (waves >= 8) { window.clearInterval(timer); setGreet(undefined); setReaction(undefined); return; }
      setGreet(waves % 2 ? 'wave-b' : 'wave-a');
    }, 220);
    return () => { window.clearInterval(timer); setGreet(undefined); };
  }, [active]);

  // ---- walking on the top edge ------------------------------------------------
  // The character walks inside its window (the window only follows Bento), moved every display frame
  // by requestAnimationFrame. The walk cycle's frame is picked from the distance covered, so the feet
  // land where they step and the cadence never stutters, even across a turn at the end of the edge.
  const trackRef = useRef<HTMLDivElement>(null);
  const edgeRef = useRef(0.8);
  const trackX = useRef(0);
  const dockRef = useRef(dock);
  dockRef.current = dock;
  const walkRef = useRef<{ raf: number; done: (finished: boolean) => void }>();
  /** How far through the walk cycle it is, in cycles; carried across turns so the cadence never restarts mid-stroll. */
  const walkPhase = useRef(0);
  const pushRegions = useRef<() => void>(() => undefined);
  const span = () => Math.max(1, window.innerWidth - 2 * DOCK_PAD - 2 * EDGE_MARGIN);
  const xOfEdge = (edge: number) => dockRef.current === 'top' ? DOCK_PAD + EDGE_MARGIN + span() * edge - HOME_X : 0;
  const applyTrack = (x: number) => {
    trackX.current = x;
    if (trackRef.current) trackRef.current.style.transform = `translate3d(${x}px, 0, 0)`;
  };
  const stopWalk = () => {
    const walk = walkRef.current;
    if (!walk) return;
    window.cancelAnimationFrame(walk.raf);
    walkRef.current = undefined;
    void invoke('pet_set_edge', { edge: edgeRef.current }).catch(() => undefined);
    walk.done(false);
  };
  /** Walks to `to` (0..1 along the edge) at `speed` logical px/s; resolves true when it got there. */
  const walkEdge = (to: number, speed: number) => new Promise<boolean>(resolve => {
    stopWalk();
    const from = edgeRef.current;
    const distance = Math.abs(to - from) * span();
    if (distance < 2) { resolve(true); return; }
    const ms = distance / speed * 1000;
    const start = performance.now();
    // Feet planted: one cycle per WALK_CYCLE_PX of ground. Running takes longer strides so the legs don't blur.
    const cyclePx = Math.max(WALK_CYCLE_PX, speed * 0.62);
    let covered = 0;
    let lastFrame = -1;
    let regionsAt = 0;
    const tick = (now: number) => {
      const t = Math.min(1, (now - start) / ms);
      edgeRef.current = from + (to - from) * t;
      applyTrack(xOfEdge(edgeRef.current));
      // The pose is picked from the ground covered, never from time: one clock moves the body and the legs.
      walkPhase.current += (distance * t - covered) / cyclePx;
      covered = distance * t;
      const frame = (WALK_STAND + Math.floor(walkPhase.current * WALK_POSES)) % WALK_POSES;
      if (frame !== lastFrame) { lastFrame = frame; setStride(frame); }
      if (now - regionsAt > 150) { regionsAt = now; pushRegions.current(); }
      if (t >= 1) {
        walkRef.current = undefined;
        void invoke('pet_set_edge', { edge: edgeRef.current }).catch(() => undefined);
        pushRegions.current();
        resolve(true);
        return;
      }
      if (walkRef.current) walkRef.current.raf = window.requestAnimationFrame(tick);
    };
    walkRef.current = { raf: window.requestAnimationFrame(tick), done: resolve };
  });
  // Not walking: feet together (the passing pose), and the next walk starts its cycle from there.
  useEffect(() => {
    if (motion === 'walk' || motion === 'run') return;
    setStride(WALK_STAND); walkPhase.current = 0;
  }, [motion]);
  // Keep the character at its spot when the dock changes or Bento is resized.
  useLayoutEffect(() => { if (!walkRef.current) applyTrack(xOfEdge(edgeRef.current)); });
  useEffect(() => {
    const onResize = () => { if (!walkRef.current) { applyTrack(xOfEdge(edgeRef.current)); pushRegions.current(); } };
    window.addEventListener('resize', onResize);
    const unEdge = listen<number>('pet-edge', ({ payload }) => { stopWalk(); edgeRef.current = payload; applyTrack(xOfEdge(payload)); pushRegions.current(); });
    return () => { window.removeEventListener('resize', onResize); void unEdge.then(unlisten => unlisten()); };
  }, []);
  // Each time it comes out on Bento, pick up the spot pet.rs chose for it.
  useEffect(() => {
    if (!ON_BENTO || !active) return;
    void invoke<number>('pet_edge').then(edge => { if (walkRef.current) return; edgeRef.current = edge; applyTrack(xOfEdge(edge)); pushRegions.current(); }).catch(() => undefined);
  }, [active, dock]);
  useEffect(() => () => stopWalk(), []);

  const urgent = view?.items.find(item => item.urgent && !((snoozed[item.id] ?? 0) > Date.now()));
  // A snooze running out brings its item back.
  useEffect(() => {
    const until = Math.min(...Object.values(snoozed).filter(at => at > Date.now()));
    if (!Number.isFinite(until)) return;
    const timer = window.setTimeout(() => setSnoozed(current => ({ ...current })), until - Date.now() + 50);
    return () => window.clearTimeout(timer);
  }, [snoozed]);
  const working = !!view?.items.some(item => item.tone === 'working');
  const calm = !view || (!urgent && !working);
  useEffect(() => { if (!calm) setCalmSince(Date.now()); }, [calm, view]);

  // Something just finished: a happy hop.
  const lastDone = useRef<number | null | undefined>(undefined);
  useEffect(() => {
    if (!view) return;
    const previous = lastDone.current;
    lastDone.current = view.doneAt;
    if (view.doneAt == null || previous === undefined || (previous != null && view.doneAt <= previous)) return;
    setReaction('happy');
    setMotion('hop');
    setSparkle(Date.now());
    setDoneFlash(true);
    const flash = window.setTimeout(() => setDoneFlash(false), 2400);
    const timer = window.setTimeout(() => { setReaction(undefined); setMotion('none'); }, 3600);
    return () => { window.clearTimeout(timer); window.clearTimeout(flash); setDoneFlash(false); };
  }, [view]);

  const lastAction = useRef<string>();
  // ---- behaviour ------------------------------------------------------------
  // What it does on its own. Anything that needs the user wins; otherwise it
  // picks something to do, weighted by where it is, the time, and how long
  // things have been quiet.
  const onLedge = dock === 'top' || dock === 'corner';
  const busy = useMemo(() => urgent ? 'urgent' : working ? 'working' : 'calm', [urgent, working]);
  useEffect(() => {
    let cancelled = false;
    let timer: number;
    const stopWalking = () => { stopWalk(); void invoke('pet_stop').catch(() => undefined); };
    const after = (ms: number, next: () => void) => { timer = window.setTimeout(() => { if (!cancelled) next(); }, ms); };

    // Tucks itself away behind the window's edge, reappears somewhere else
    // (`side` forces where), and comes back out there.
    const relocate = (side: string | null, then: () => void) => {
      setAway(true);
      setProp(undefined);
      after(side ? 280 : 420, () => {
        void invoke<boolean>('pet_jump', { to: null, side }).then(moved => {
          if (cancelled) return;
          after(moved ? 160 : 0, () => { setAway(false); then(); });
        }).catch(() => { setAway(false); after(1000, then); });
      });
    };

    // Hidden (the other pet is out): stay still until shown again.
    if (!active) {
      stopWalking();
      setPose('sit');
      setMotion('none');
      setProp(undefined);
      setRestFace('idle');
      return () => { cancelled = true; window.clearTimeout(timer); };
    }

    if (!onLedge) {
      const hanging = dock === 'bottom';
      setPose(hanging ? 'hang' : 'peek');
      // Something needs the user: back up to the top edge where it is seen.
      if (busy === 'urgent' && dock !== 'inside') {
        relocate('top', () => undefined);
        return () => { cancelled = true; window.clearTimeout(timer); };
      }
      const loop = () => {
        const sleepy = busy === 'calm' && (Date.now() - calmSince > SLEEP_AFTER_MS || lateNight());
        const recentPointer = Date.now() - pointerMovedAt.current < 3000;
        setRestFace(busy === 'working' ? 'working' : sleepy ? 'sleep' : 'idle');
        setAutoGaze(undefined);
        if (busy === 'urgent') { setMotion('hop'); after(5000, loop); return; }
        const action = pick<string>([
          ['still', 2],
          ['look', 2],
          ['lean', hanging ? 0 : 2],
          ['duck', hanging ? 0 : 1.4],
          ['swing', hanging ? 3 : 0],
          ['kick', hanging ? 1.5 : 0],
          ['peekin', 1.5],
          ['move', dock !== 'inside' && !recentPointer ? (sleepy ? 0.4 : 1.6) : 0],
          ['hide', dock !== 'inside' && !recentPointer && !sleepy ? 0.6 : 0],
        ]);
        if (action === 'move') { relocate(null, () => after(random(4000, 9000), loop)); return; }
        if (action === 'hide') {
          // Hide and seek: gone for a while, then out somewhere else.
          setAway(true);
          after(random(10_000, 30_000), () => relocate(null, () => {
            setRestFace('happy');
            setMotion(hanging ? 'swing' : 'lean');
            after(1800, () => { setRestFace('idle'); after(random(3000, 7000), loop); });
          }));
          return;
        }
        if (action === 'duck') {
          // Pops back behind the edge for a moment, then out again.
          setMotion('duck');
          after(1600, () => { setMotion('none'); after(random(4000, 9000), loop); });
          return;
        }
        if (action === 'peekin') {
          // Looks into Bento.
          setRestFace('curious');
          setMotion(hanging ? 'none' : 'lean');
          setAutoGaze(hanging ? [random(-0.4, 0.4), -0.9] : [dock === 'right' ? -0.95 : 0.95, random(-0.2, 0.4)]);
          after(random(3000, 5500), () => { setAutoGaze(undefined); setRestFace('idle'); setMotion('none'); after(random(3000, 7000), loop); });
          return;
        }
        if (action === 'look') {
          setMotion('look');
          const glance = (left: number) => {
            if (left <= 0) { setAutoGaze(undefined); after(random(2000, 5000), loop); return; }
            setAutoGaze([random(-1, 1), random(-0.6, 0.5)]);
            after(random(900, 1800), () => glance(left - 1));
          };
          glance(Math.round(random(2, 4)));
          return;
        }
  
        setMotion(action === 'swing' ? 'swing' : action === 'lean' ? 'lean' : hanging ? 'swing' : 'none');
        after(random(6000, 12_000), loop);
      };
      loop();
      return () => { cancelled = true; window.clearTimeout(timer); setAway(false); };
    }

    if (busy === 'urgent') {
      stopWalking();
      setPose('perch');
      setRestFace('idle');
      const nudge = () => { setMotion('hop'); after(900, () => { setMotion('none'); after(5200, nudge); }); };
      nudge();
      return () => { cancelled = true; window.clearTimeout(timer); };
    }

    // In the corner it keeps doing its own things while work runs; on Bento it watches.
    if (busy === 'working' && dock !== 'corner') {
      // Watching its work: sits on the edge and looks down into the window.
      stopWalking();
      setPose('sit');
      setRestFace('working');
      setMotion('sway');
      return () => { cancelled = true; window.clearTimeout(timer); };
    }

    let rested = false;
    const step = () => {
      const quietFor = Date.now() - calmSince;
      const sleepy = quietFor > SLEEP_AFTER_MS || lateNight();
      const onTop = dock === 'top';
      // Its own little pastimes. Mostly in the corner, where it spends its day.
      const corner = dock === 'corner';
      // The top edge is long: it always strolls there, even while the pointer moves about in Bento.
      const roam = onTop;
      // Takes turns rather than rolling dice, so every one of them shows up:
      // a shuffled round of all of them, then a new round.
      // In the corner every pastime starts and ends in the plain seated pose, and it rests there a while
      // (breathing, glancing about) before picking the next one. That pose is where it can be poked.
      // Round 10: the corner is a showcase of pastimes, one straight after another, so it no longer rests
      // in the seated pose between them (CORNER_REST). The shuffled rounds above still decide the order.
      if (corner && !rested && CORNER_REST) {
        rested = true;
        setPose('sit');
        setProp(undefined);
        setMotion('breathe');
        setRestFace(sleepy ? 'sleep' : 'idle');
        setAtBase(true);
        const until = Date.now() + (sleepy ? random(25_000, 45_000) : random(7000, 14_000));
        const idle = () => {
          // Being played with keeps it here.
          if (Date.now() >= until && Date.now() - pokedAt.current > 5000) { setAtBase(false); setAutoGaze(undefined); step(); return; }
          // Fond of us: now and then it waves on its own.
          if (!sleepy && affection.current >= 60 && Math.random() < 0.12) {
            setAutoGaze(undefined);
            setRestFace('happy');
            const wave = (left: number) => {
              if (left <= 0) { setPose('sit'); setRestFace('idle'); after(random(1400, 2400), idle); return; }
              setPose(left % 2 ? 'wave-a' : 'wave-b');
              after(230, () => wave(left - 1));
            };
            wave(6);
            return;
          }
          setAutoGaze(Math.random() < 0.35 ? undefined : [random(-1, 1), random(-0.6, 0.5)]);
          after(random(1400, 2600), idle);
        };
        after(random(800, 1500), idle);
        return;
      }
      rested = false;
      const action = nextInRound(corner ? pastimesNow() : onTop && roam ? WORKOUTS : ['look'], lastAction.current);
      lastAction.current = action;
      setAutoGaze(undefined);
      setProp(undefined);
      // Straight on to the next thing: no empty stretch of just changing faces in between.
      const done = (face: Face = 'idle') => {
        setMotion('none'); setRestFace(face);
        // In the corner the next pastime starts at once, in the same render (no in-between frame).
        if (corner) { step(); return; }
        after(random(900, 1600), step);
      };
      // Walks along the top edge to `to` (0..1) at `speed` px/s; then calls `next`.
      const walkTo = (to: number, speed: number, gait: 'walk' | 'run', next: () => void) => {
        if (dock !== 'top') { next(); return; }
        const from = edgeRef.current;
        setWalkDir(to > from ? 1 : -1);
        // It walks on its legs (the walk sheet), not by sliding the perched pose along.
        setPose('walk');
        setMotion(gait);
        void walkEdge(to, speed).then(finished => { if (cancelled || !finished) return; setMotion('none'); setWalkMs(0); next(); });
      };
      if (action === 'wander') {
        // Life on the top edge: a few steps somewhere (never the whole edge in one go),
        // then a break: stands and looks round, grooves to the music, waves at us,
        // stretches or hops, or sits on the border swinging its legs. Headphones on
        // by default. After a while it gets tired and has a longer sit.
        const tiredAt = Date.now() + random(60_000, 120_000);
        const speed = WALK_SPEED * random(0.95, 1.05);
        // Its face keeps changing while it is up and about.
        let strolling = true;
        const mood = () => {
          if (cancelled || !strolling) return;
          setRestFace(pick<Face>([['content', 3], ['happy', 3], ['smug', 1], ['love', 1], ['idle', 1]]));
          window.setTimeout(mood, random(2500, 5500));
        };
        mood();
        const stand = () => { setPose('walk-hp'); setProp('sing'); setMotion('none'); setWalkMs(0); };
        // Sits on the border facing us, legs dangling, and swings them now and then.
        const sit = (ms: number, next: () => void) => {
          strolling = false;
          stopWalking();
          setWalkMs(0);
          setProp(undefined);
          setMotion('none');
          setPose('edge-sit');
          setRestFace(pick<Face>([['content', 2], ['idle', 2], ['happy', 1]]));
          const until = Date.now() + ms;
          const swing = () => {
            if (Date.now() >= until) { strolling = true; mood(); next(); return; }
            after(random(2500, 6000), () => {
              const frames: Pose[] = [];
              const cycles = Math.round(random(2, 4));
              for (let index = 0; index < cycles; index += 1) frames.push('edge-kick-l', 'edge-kick-r', 'edge-kick-l', 'edge-sit');
              const play = (index: number) => {
                if (index >= frames.length) { setRestFace(pick<Face>([['content', 1], ['idle', 2]])); swing(); return; }
                setPose(frames[index]);
                if (index === 0) setRestFace('happy');
                after(frames[index] === 'edge-kick-r' ? 300 : 200, () => play(index + 1));
              };
              play(0);
            });
          };
          swing();
        };
        // Which way it is heading: it keeps going that way after a break and only
        // turns round near an end of the edge.
        let heading: 1 | -1 = 1;
        // A break between strolls. It never moves during one: grooves to the music, or sits on the border.
        const rest = (next: () => void) => {
          stopWalking();
          setWalkMs(0);
          const kind = pick<string>([['music', 3], ['sit', 2]]);
          if (kind === 'sit') { sit(random(8000, 18_000), next); return; }
          // Grooves to the beat, notes floating up.
          setPose('music'); setProp('sing'); setMotion('beat'); setRestFace(pick<Face>([['music', 3], ['content', 2], ['happy', 1]]));
          after(random(4000, 9000), () => { stand(); after(500, next); });
        };
        // A stroll: it keeps walking for a few stretches, turning round at the ends of the edge
        // without stopping, then takes a break. No standing about in the middle of a stroll.
        const stroll = () => {
          if (Date.now() >= tiredAt) { sit(random(25_000, 60_000), () => done('idle')); return; }
          let stretches = Math.round(random(2, 4));
          setPose('walk-hp');
          setProp('sing');
          setMotion('walk');
          const go = () => {
            if (cancelled) return;
            if (stretches <= 0) { setMotion('none'); setWalkMs(0); rest(stroll); return; }
            stretches -= 1;
            const from = edgeRef.current;
            if (from > 0.9) heading = -1;
            else if (from < 0.1) heading = 1;
            const to = Math.max(0.03, Math.min(0.97, from + heading * random(0.25, 0.5)));
            setWalkDir(to > from ? 1 : -1);
            void walkEdge(to, speed).then(finished => { if (finished) go(); });
          };
          go();
        };
        // Says hi first, standing still; then it may start with a break rather than walking straight off.
        const begin = () => {
          void invoke<number>('pet_edge').then(from => {
            if (cancelled) return;
            if (!walkRef.current) { edgeRef.current = from; applyTrack(xOfEdge(from)); }
            heading = edgeRef.current < 0.5 ? 1 : -1;
            setWalkDir(heading);
            if (Math.random() < 0.3) rest(stroll); else stroll();
          }).catch(() => done());
        };
        stopWalking();
        setPose('walk-hp');
        setMotion('none');
        after(Math.max(0, greetUntil.current - Date.now()), begin);
        return;
      }
      if (action === 'stroll') {
        // A wander to somewhere along the edge, a look round, maybe on again.
        setRestFace('idle');
        const leg = (left: number) => {
          if (left <= 0) { done(); return; }
          walkTo(random(0.08, 0.92), WALK_SPEED * random(0.9, 1.08), 'walk', () => {
            setPose('perch');
            setMotion('look');
            setAutoGaze([random(-1, 1), random(-0.3, 0.6)]);
            after(random(1500, 3500), () => { setAutoGaze(undefined); setMotion('none'); leg(left - 1); });
          });
        };
        leg(Math.round(random(1, 3)));
        return;
      }
      if (action === 'jog') {
        // Laps from one end of the edge to the other, then a breather.
        setRestFace('content');
        void invoke<number>('pet_edge').then(start => {
          const lap = (left: number, toRight: boolean) => {
            if (left <= 0) {
              setPose('sit');
              setRestFace('dizzy');
              setMotion('breathe');
              after(2200, () => done('happy'));
              return;
            }
            walkTo(toRight ? 0.94 : 0.06, random(110, 150), 'run', () => {
              setMotion('hop');
              after(500, () => lap(left - 1, !toRight));
            });
          };
          lap(Math.round(random(2, 4)), start < 0.5);
        }).catch(() => done());
        return;
      }
      if (action === 'exercise') {
        // A short workout on the spot: squats, jumps, side bends.
        setPose('perch');
        setRestFace('cool');
        const moves = ([['squat', 1], ['jump', 1], ['bend', 1]] as [Motion, number][])
          .sort(() => Math.random() - 0.5).slice(0, Math.round(random(1, 3))).map(([move]) => move);
        const next = (index: number) => {
          if (index >= moves.length) { setMotion('none'); setRestFace('happy'); setSparkle(Date.now()); after(1400, () => done('smug')); return; }
          setMotion(moves[index]);
          setRestFace(index % 2 ? 'angry' : 'cool');
          after(random(3200, 4800), () => { setMotion('none'); setRestFace('content'); after(800, () => next(index + 1)); });
        };
        after(600, () => next(0));
        return;
      }
      // Each pastime has an entrance and an exit, so one flows into the next:
      // it stands, its prop appears in its hands (a little sparkle), it sits
      // down to it; afterwards the prop vanishes and it stands back up.
      const enter = (carry: Pose[], then: () => void) => {
        // In the corner it cuts straight into the pastime: no reach/stand frame, no sparkle.
        if (corner) {
          setMotion('none');
          setRestFace('idle');
          setPose(carry[0]);
          const go = (index: number) => {
            if (index >= carry.length) { after(900, then); return; }
            after(900, () => { setPose(carry[index]); go(index + 1); });
          };
          go(1);
          return;
        }
        // Hands come up first (the in-between frame), then the prop appears in them.
        setPose(corner ? 'reach' : 'stand');
        setMotion('none');
        setRestFace('idle');
        const next = (index: number) => {
          if (index >= carry.length) { after(900, then); return; }
          after(index === 0 ? 600 : 900, () => { setSparkle(Date.now()); setPose(carry[index]); next(index + 1); });
        };
        next(0);
      };
      const leave = (face: Face, steps: Pose[] = ['sit']) => {
        setProp(undefined);
        setAutoGaze(undefined);
        // In the corner there is no exit: the next pastime cuts in directly.
        if (corner) { done(face); return; }
        const next = (index: number) => {
          if (index >= steps.length) {
            after(700, () => {
              if (steps.includes('stand-hat')) setSparkle(Date.now());
              // Puts its hands down through the in-between frame.
              if (corner) { setPose('reach'); after(380, () => { setPose('stand'); after(500, () => done(face)); }); return; }
              setPose('stand');
              after(500, () => done(face));
            });
            return;
          }
          setSparkle(Date.now());
          setPose(steps[index]);
          after(800, () => next(index + 1));
        };
        next(0);
      };
      if (action === 'music') {
        // Headphones on, bobbing along to the beat with notes floating up.
        enter(['music'], () => {
          setPose('music');
          setRestFace('music');
          setMotion('beat');
          setProp('sing');
          const groove = (left: number) => {
            if (left <= 0) { setMotion('none'); setProp(undefined); after(600, () => leave('content')); return; }
            setRestFace(pick<Face>([['music', 3], ['content', 2], ['happy', 2], ['squint', 1]]));
            after(random(3000, 5000), () => groove(left - 1));
          };
          groove(Math.round(random(3, 5)));
        });
        return;
      }
      if (action === 'stretch') {
        // A big stretch: arms up, legs out, a long yawn; then a contented slump back.
        setPose('stretch-up');
        setRestFace('yawn');
        setMotion('stretch');
        after(1500, () => {
          setMotion('none');
          after(900, () => { setRestFace('squint'); after(700, () => { setPose('sit'); done('content'); }); });
        });
        return;
      }
      if (action === 'cube') {
        // Rubik's cube: frowns at it, twists (small tilts), stuck for a bit, then solves it and shows it off.
        enter(['cube-a'], () => {
          const twist = (left: number) => {
            if (left <= 0) {
              setPose('cube-b');
              setRestFace('happy');
              setMotion('hop');
              after(1800, () => { setMotion('none'); setRestFace('smug'); after(900, () => leave('happy')); });
              return;
            }
            setRestFace(pick<Face>([['think', 3], ['pout', 1], ['curious', 1]]));
            setMotion('tilt');
            after(random(1600, 2600), () => { setMotion('none'); after(400, () => twist(left - 1)); });
          };
          twist(Math.round(random(2, 4)));
        });
        return;
      }
      if (action === 'doze') {
        // Dozes off on the stool: arms hang, head nods, Zz floats up; jolts awake, then dozes again or gets up.
        setPose('doze');
        setRestFace('sleep');
        const nap = (left: number) => {
          setMotion('nod');
          setProp('zz');
          after(random(5000, 8000), () => {
            setProp(undefined);
            setMotion('none');
            setRestFace('surprised');
            after(800, () => {
              if (left <= 1) { setRestFace('yawn'); after(1200, () => { setPose('sit'); done('squint'); }); return; }
              setRestFace('sleep');
              after(600, () => nap(left - 1));
            });
          });
        };
        nap(Math.round(random(1, 2)));
        return;
      }
      if (action === 'bubbles') {
        // Blows bubbles: dips the wand, lifts it to the visor and blows; the bubbles drift up and pop.
        enter(['bubble-b'], () => {
          const blow = (left: number) => {
            if (left <= 0) { setProp(undefined); leave('content'); return; }
            setPose('bubble-b');
            setRestFace('curious');
            after(1100, () => {
              setPose('bubble-a');
              setRestFace('pout');
              setProp(undefined);
              after(60, () => setProp('bubbles'));
              after(900, () => {
                setRestFace(pick<Face>([['happy', 3], ['love', 1]]));
                after(random(2200, 3000), () => blow(left - 1));
              });
            });
          };
          blow(Math.round(random(2, 4)));
        });
        return;
      }
      if (action === 'plane') {
        // Folds a paper plane, aims, throws: the plane glides off; it watches it go.
        enter(['plane-a'], () => {
          setRestFace('think');
          setAutoGaze([-0.8, -0.5]);
          after(1500, () => {
            setPose('plane-b');
            setRestFace('surprised');
            setProp('plane');
            after(500, () => {
              setRestFace('happy');
              after(PLANE_MS - 500, () => { setProp(undefined); setAutoGaze(undefined); setPose('sit'); done(pick<Face>([['happy', 2], ['smug', 1]])); });
            });
          });
        });
        return;
      }
      if (action === 'snack') {
        // An onigiri: looks at it, takes bites (chewing bob), happy between bites.
        enter(['snack-a'], () => {
          const bite = (left: number) => {
            if (left <= 0) { setMotion('none'); leave('content'); return; }
            setPose('snack-a');
            setRestFace('curious');
            setMotion('none');
            after(1200, () => {
              setPose('snack-b');
              setRestFace('squint');
              setMotion('bob');
              after(random(1800, 2600), () => { setMotion('none'); setRestFace('happy'); after(1200, () => bite(left - 1)); });
            });
          };
          bite(Math.round(random(2, 3)));
        });
        return;
      }
      if (action === 'scarf') {
        // Winter: it shakes out its scarf, wraps it on and tugs it up snug (cosy, blushing),
        // then holds out a mitten and a snowflake drifts down onto it: surprise, then delight.
        enter(['scarf-hold'], () => {
          setRestFace('curious');
          after(1600, () => {
            setPose('scarf-tug');
            setRestFace('content');
            setMotion('wiggle');
            after(2400, () => {
              setMotion('none');
              setRestFace('happy');
              after(1400, () => {
                setPose('scarf-catch');
                setRestFace('curious');
                setAutoGaze([-0.9, -0.2]);
                setProp('flake');
                after(SNOWFLAKE_MS, () => {
                  setRestFace('surprised');
                  after(900, () => {
                    setRestFace('happy');
                    after(2000, () => { setProp(undefined); setAutoGaze(undefined); setPose('scarf-tug'); setRestFace('content'); after(1800, () => leave('content')); });
                  });
                });
              });
            });
          });
        });
        return;
      }
      if (action === 'drink') {
        // Warm drink: blows on it, sips (mug up to its face), a contented sigh.
        enter(['stand-mug'], () => {
          setPose('mug');
          // No steam here: the mug sits in front of the face screen, so steam would cover the face.
          const sip = (left: number) => {
            if (left <= 0) { leave('content'); return; }
            setRestFace('pout');
            after(1200, () => {
              setPose('mug-sip');
              setRestFace('squint');
              after(1800, () => {
                setPose('mug');
                setRestFace('content');
                after(1400, () => { setRestFace('idle'); after(random(2500, 4500), () => sip(left - 1)); });
              });
            });
          };
          after(1200, () => sip(Math.round(random(2, 3))));
        });
        return;
      }
      if (action === 'code') {
        // Coding: comes over holding the laptop, sits, opens it and types in
        // bursts, gets stuck, carries on. Only the paw tips press the keys:
        // arms, body and face never move.
        enter(['stand-laptop'], () => {
          setPose('type-a');
          let typing = false;
          const tap = (left: boolean) => {
            if (cancelled || !typing) return;
            setPose(left ? 'type-l' : 'type-r');
            window.setTimeout(() => { if (!cancelled && typing) setPose('type-a'); }, random(70, 100));
            window.setTimeout(() => tap(Math.random() < 0.75 ? !left : left), random(140, 240) + (Math.random() < 0.1 ? 320 : 0));
          };
          const bursts = (left: number) => {
            if (left <= 0) {
              typing = false;
              setPose('type-a');
              setRestFace('happy');
              after(1400, () => leave('smug'));
              return;
            }
            setRestFace('typing');
            setProp('code');
            typing = true;
            tap(true);
            after(random(3500, 6000), () => {
              typing = false;
              setPose('type-a');
              setProp(undefined);
              setRestFace(pick<Face>([['think', 4], ['pout', 1]]));
              after(random(1800, 3000), () => bursts(left - 1));
            });
          };
          // First it thinks about what to write, then starts typing.
          setRestFace('think');
          after(random(1800, 2600), () => bursts(Math.round(random(2, 4))));
        });
        return;
      }
      if (action === 'read') {
        enter(['stand-book'], () => {
          setPose('book');
          setAutoGaze([0.45, 0.7]);
          const pages = (left: number) => {
            if (left <= 0) { leave(pick<Face>([['content', 2], ['love', 1]])); return; }
            after(random(4000, 7000), () => {
              // Turns a page: the right mitten pinches the right page's edge, the sheet rises,
              // stands up over the spine and settles on the left; then it reads on.
              setPageTurn(Date.now());
              after(PAGE_TURN_MS, () => {
                setPageTurn(undefined);
                const twist = Math.random() < 0.3;
                if (twist) setRestFace(pick<Face>([['surprised', 2], ['shy', 1]]));
                after(twist ? 1600 : 300, () => { setRestFace('idle'); pages(left - 1); });
              });
            });
          };
          pages(Math.round(random(3, 5)));
        });
        return;
      }
      if (action === 'water') {
        // Fetches the watering can, crouches by its plant (side-on) and waters it, humming.
        enter(['stand-can'], () => {
          setPose('water');
          setRestFace('happy');
          setProp('sing');
          const pour = (left: number) => {
            if (left <= 0) {
              setProp('hearts');
              setRestFace('love');
              after(1800, () => leave('content', ['stand-can']));
              return;
            }
            setPose('water-pour');
            setProp('pour');
            after(random(2600, 3600), () => {
              setPose('water');
              setProp('sing');
              after(random(1200, 2000), () => pour(left - 1));
            });
          };
          after(1400, () => pour(Math.round(random(2, 3))));
        });
        return;
      }
      if (action === 'sword') {
        // Chinese wuxia roleplay: a wide flat straw hat and a toy wooden jian with a red tassel.
        // It salutes us (抱拳) facing front, then mixes side-on thrusts with front-facing wrist
        // flourishes (挽剑花), and salutes again before it all goes.
        enter(['stand-hat', 'sword-salute'], () => {
          setRestFace('cool');
          setMotion('none');
          // 挽剑花: body and hat hold perfectly still; only the blade circles the wrist.
          const flourish = (then: () => void) => {
            setMotion('still');
            setPose('sword-spin');
            setProp('blade');
            after(random(1800, 2600), () => { setProp(undefined); setMotion('none'); after(350, then); });
          };
          const thrust = (then: () => void) => {
            setPose('sword');
            setRestFace('cool');
            after(random(500, 800), () => {
              setPose('sword-raise');
              setRestFace('angry');
              after(random(450, 700), () => {
                setPose('sword-cut');
                setProp('slash');
                after(520, () => { setProp(undefined); after(random(400, 700), then); });
              });
            });
          };
          const form = (left: number, front: boolean) => {
            if (left <= 0) {
              setPose('sword-salute');
              setRestFace('smug');
              setProp('petals');
              after(2200, () => leave('cool', ['stand-hat']));
              return;
            }
            if (front) {
              setRestFace(pick<Face>([['cool', 2], ['happy', 1]]));
              flourish(() => form(left - 1, !front));
            } else {
              thrust(() => form(left - 1, !front));
            }
          };
          after(1600, () => form(Math.round(random(3, 5)), Math.random() < 0.5));
        });
        return;
      }
      if (action === 'hide') {
        // Hide and seek: gone for a while, then pops out somewhere else.
        setAway(true);
        after(random(10_000, 30_000), () => relocate(null, () => {
          setPose('perch');
          setRestFace('happy');
          setMotion('wiggle');
          after(1200, () => { setMotion('none'); setRestFace('idle'); after(random(4000, 8000), step); });
        }));
        return;
      }
      if (action === 'prone') {
        // Flops over the edge, arms dangling.
        setPose('prone');
        setMotion('none');
        setRestFace(sleepy ? 'sleep' : 'idle');
        after(sleepy ? random(30_000, 70_000) : random(12_000, 24_000), step);
        return;
      }
      if (action === 'kick') {
        // Sits on the edge swinging its legs.
        setPose('sit');
        setRestFace('happy');
        setMotion('kick');
        after(random(3500, 6000), () => { setMotion('none'); setRestFace('idle'); after(random(4000, 8000), step); });
        return;
      }



      if (action === 'move') {
        setAway(true);
        after(260, () => {
          void invoke<boolean>('pet_jump').then(moved => {
            if (cancelled) return;
            setPose(pick<Pose>([['sit', 3], ['perch', 2]]));
            setMotion('none');
            setRestFace('idle');
            after(moved ? 140 : 0, () => { setAway(false); after(random(8000, 16_000), step); });
          }).catch(() => { setAway(false); after(1000, step); });
        });
        return;
      }
      // A breather: sits and glances around.
      setPose('sit');
      setMotion('none');
      setRestFace(sleepy ? 'sleep' : 'idle');
      const glance = (left: number) => {
        if (left <= 0) { setAutoGaze(undefined); after(random(2000, 5000), step); return; }
        setAutoGaze([random(-1, 1), random(-0.6, 0.5)]);
        after(random(900, 1800), () => glance(left - 1));
      };
      glance(Math.round(random(2, 4)));
    };
    step();
    return () => { cancelled = true; window.clearTimeout(timer); setAway(false); stopWalking(); setWalkMs(0); setAtBase(false); setPageTurn(undefined); };
    // calmSince only matters when the next action is chosen. Each time this pet is
    // shown again it starts afresh, so it never carries on with what it was doing hidden.
  }, [onLedge, dock, busy, active, resetKey]);

  // Hovering wakes it up and makes it look at you.
  useEffect(() => {
    if (!hover) return;
  }, [hover, pose]);

  // Blink every few seconds, sometimes twice.
  useEffect(() => {
    let timer: number;
    const next = () => {
      timer = window.setTimeout(() => {
        setBlink(true);
        window.setTimeout(() => setBlink(false), 130);
        if (Math.random() < 0.2) window.setTimeout(() => { setBlink(true); window.setTimeout(() => setBlink(false), 120); }, 300);
        next();
      }, random(2600, 6400));
    };
    next();
    return () => window.clearTimeout(timer);
  }, []);

  // ---- speech -----------------------------------------------------------------
  // Speaks only when something happens. Urgent items stay up until resolved;
  // new events are queued and shown once each; a tip now and then otherwise.
  const seen = useRef<Map<string, string>>();
  const queue = useRef<Bubble[]>([]);
  const [announcing, setAnnouncing] = useState<Bubble>();
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!view) return;
    const known = seen.current;
    const next = new Map(view.items.map(item => [item.id, item.text]));
    if (known) {
      for (const item of view.items) {
        // Finished work is pushed at once.
        if (item.urgent || item.tone !== 'done' || known.has(item.id)) continue;
        queue.current.push({ key: item.id, text: item.text, tone: item.tone, item });
      }
    }
    seen.current = next;
    setTick(value => value + 1);
  }, [view]);

  useEffect(() => {
    if (!active) { queue.current = []; return; }
    if (announcing || !queue.current.length) return;
    const next = queue.current.shift()!;
    setAnnouncing(next);
    const timer = window.setTimeout(() => setAnnouncing(undefined), ANNOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [announcing, tick, active]);

  // A tip once in a while (or a nudge to rest late at night). Running projects
  // are not announced; they are in the menu.
  const [tip, setTip] = useState<Bubble>();
  const nightNoted = useRef('');
  const viewRef = useRef(view);
  viewRef.current = view;
  useEffect(() => {
    let timer: number;
    let hide: number;
    const schedule = (ms: number) => {
      timer = window.setTimeout(() => {
        const today = new Date().toDateString();
        if (lateNight() && nightNoted.current !== today) {
          nightNoted.current = today;
          setTip({ key: 'night', text: `已经 ${new Date().getHours()} 点了，记得早点休息`, tone: 'tip' });
        } else {
          setTip({ key: `tip-${Date.now()}`, text: TIPS[Math.floor(Math.random() * TIPS.length)], tone: 'tip' });
        }
        hide = window.setTimeout(() => setTip(undefined), 5000);
        schedule(random(6, 10) * 60_000);
      }, ms);
    };
    schedule(random(2, 4) * 60_000);
    return () => { window.clearTimeout(timer); window.clearTimeout(hide); };
  }, []);

  // Only the pet on Bento keeps the clock: it is shown exactly while Bento is in front.
  const [rest, setRest] = useState<Bubble>();
  const session = useRef<{ since: number; leftAt: number | null }>({ since: Date.now(), leftAt: null });
  useEffect(() => {
    if (!ON_BENTO) return;
    const now = Date.now();
    const clock = session.current;
    if (active) {
      if (clock.leftAt != null && now - clock.leftAt >= BREAK_RESET_MS) clock.since = now;
      clock.leftAt = null;
    } else {
      clock.leftAt = now;
      setRest(undefined);
    }
  }, [active]);
  useEffect(() => {
    if (!ON_BENTO || !active) return;
    let hide: number | undefined;
    const check = window.setInterval(() => {
      const clock = session.current;
      if (Date.now() - clock.since < BREAK_AFTER_MS) return;
      const hours = Math.round((Date.now() - clock.since) / (60 * 60_000));
      clock.since = Date.now();
      setRest({ key: `rest-${Date.now()}`, text: `已经连续用了 ${Math.max(1, hours)} 小时，起来活动一下吧`, tone: 'tip' });
      setReaction('happy');
      setMotion('stretch');
      window.setTimeout(() => { setReaction(undefined); setMotion('none'); }, 1600);
      window.clearTimeout(hide);
      hide = window.setTimeout(() => setRest(undefined), BREAK_SHOW_MS);
    }, 30_000);
    return () => { window.clearInterval(check); window.clearTimeout(hide); };
  }, [active]);

  const summary = useMemo<Bubble | undefined>(() => {
    if (!view) return undefined;
    const items = view.items;
    if (items.length) return { key: 'hover-' + items[0].id, text: items.length > 1 ? `${items[0].text} 等 ${items.length} 条` : items[0].text, tone: items[0].tone, item: items[0] };
    return undefined;
  }, [view]);

  useEffect(() => {
    const quiet = view?.quiet && !hover;
    const pinned = urgent && { key: urgent.id, text: urgent.text, tone: urgent.tone, item: urgent } as Bubble;
    const next = !active ? undefined : pinned || (hover ? summary : undefined) || (quiet ? undefined : announcing)
      || (quiet ? undefined : rest) || (quiet || dock === 'inside' ? undefined : tip);
    setBubble(current => current?.key === next?.key && current?.text === next?.text ? current : next);
  }, [active, urgent, hover, summary, announcing, rest, tip, view?.quiet, dock]);

  // The bubble types itself out, a character at a time.
  const [typed, setTyped] = useState(0);
  const bubbleText = bubble?.text ?? '';
  useEffect(() => {
    if (!bubbleText) { setTyped(0); return; }
    if (reducedMotion()) { setTyped(bubbleText.length); return; }
    setTyped(0);
    let count = 0;
    const timer = window.setInterval(() => {
      count += 1;
      setTyped(count);
      if (count >= bubbleText.length) window.clearInterval(timer);
    }, 32);
    return () => window.clearInterval(timer);
  }, [bubble?.key, bubbleText]);
  const typedAll = typed >= bubbleText.length;
  // 打开 takes you to the item in Bento; 稍后 puts it off (a waiting item for ten minutes).
  useEffect(() => { if (!active) { hoverMuted.current = false; setHovering(false); } }, [active]);
  const openItem = (item?: Item) => {
    muteHover();
    setAnnouncing(undefined);
    void invoke('pet_action', { action: 'open', projectId: item?.projectId ?? null, slot: item?.slot ?? null }).catch(() => undefined);
  };
  const putOff = (shown: Bubble) => {
    if (shown.key.startsWith('hover-')) muteHover();
    if (shown.item?.urgent) setSnoozed(current => ({ ...current, [shown.item!.id]: Date.now() + 10 * 60_000 }));
    else if (shown.key.startsWith('rest-')) setRest(undefined);
    else if (announcing?.key === shown.key) setAnnouncing(undefined);
    else setTip(undefined);
  };

  // A project folder dropped on the corner pet: it catches it and opens it in Bento.
  const sayBriefly = (text: string) => {
    setTip({ key: `say-${Date.now()}`, text, tone: 'tip' });
    window.setTimeout(() => setTip(current => current?.key.startsWith('say-') ? undefined : current), 4000);
  };
  const sayRef = useRef(sayBriefly);
  sayRef.current = sayBriefly;
  useEffect(() => {
    if (ON_BENTO) return;
    let hovering = false;
    const over = (position: { x: number; y: number }) => {
      const box = bodyRef.current?.getBoundingClientRect();
      if (!box) return false;
      const x = position.x / window.devicePixelRatio;
      const y = position.y / window.devicePixelRatio;
      return x >= box.left - 12 && x <= box.right + 12 && y >= box.top - 12 && y <= box.bottom + 12;
    };
    const take = async (path: string) => {
      setReaction('happy');
      setSparkle(Date.now());
      window.setTimeout(() => setReaction(undefined), 1600);
      try {
        let found = await invoke<Inspection>('inspect_project_dir', { path, excludeId: null });
        // A file: open the project it belongs to, if it sits right in one.
        if (found.exists && !found.isDir) {
          found = await invoke<Inspection>('inspect_project_dir', { path: path.replace(/[/\\][^/\\]+$/u, ''), excludeId: null });
          if (!found.existingProject) { sayRef.current('只能接住项目文件夹哦'); return; }
        }
        if (!found.exists) { sayRef.current('这个文件夹找不到了'); return; }
        if (found.existingProject) {
          await invoke('pet_action', { action: 'open', projectId: found.existingProject.id, slot: null });
          return;
        }
        await invoke('pet_action', { action: 'open', projectId: null, slot: null });
        window.setTimeout(() => { void emitTo('main', 'bento-pet-drop', found).catch(() => undefined); }, 400);
      } catch {
        sayRef.current('没能打开这个文件夹');
      }
    };
    const stop = getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (payload.type === 'leave') {
        if (hovering) { hovering = false; setCatching(false); setReaction(undefined); }
        return;
      }
      const inside = over(payload.position);
      if (payload.type === 'enter' || payload.type === 'over') {
        if (inside !== hovering) { hovering = inside; setCatching(inside); setReaction(inside ? 'surprised' : undefined); }
        return;
      }
      hovering = false;
      window.setTimeout(() => setCatching(false), 500);
      if (!inside || !payload.paths.length) { setReaction(undefined); return; }
      void take(payload.paths[0]);
    });
    return () => { void stop.then(unlisten => unlisten()); };
  }, []);

  // ---- face ---------------------------------------------------------------------
  // While an AI CLI is thinking it thinks along: eyes up to the side and a thought bubble over its head.
  const face: Face = reaction ?? (urgent ? (urgent.tone === 'error' ? 'error' : 'alert') : (view?.thinking ?? 0) > 0 ? 'think' : hover ? 'curious' : restFace);
  const k = SCALE[dock];
  const shownPose: Pose = catching ? 'reach' : greet ?? pose;
  const frame = frameOf(shownPose, stride, dock);
  const stool = STOOL.has(frame);
  const walking = shownPose === 'walk' || shownPose === 'walk-hp';
  const imgRef = useRef<HTMLImageElement>(null);
  const shownGroup = groupOf(frame);
  const lastGroup = useRef(shownGroup);
  useLayoutEffect(() => {
    if (lastGroup.current === shownGroup) return;
    const fromWalk = lastGroup.current === 'walk';
    lastGroup.current = shownGroup;
    // Starting or stopping a walk is a plain cut: a squash-and-fade here read as a ghost of the legs.
    if (fromWalk || shownGroup === 'walk') return;
    // In the corner one pastime cuts straight to the next (round 10): no squash between them.
    if (dock === 'corner') return;
    if (reducedMotion() || !imgRef.current?.animate) return;
    imgRef.current.animate([
      { transform: 'translateY(1.5%) scale(1.03, .96)', opacity: 0.55 },
      { transform: 'none', opacity: 1 },
    ], { duration: 260, easing: 'cubic-bezier(.3, .7, .4, 1)' });
  }, [shownGroup]);
  const spec = useMemo(() => {
    const base = IMAGES[frame] ?? IMAGES.sit;
    return { ...base, w: base.w * k, h: base.h * k, ledge: base.ledge * k, screen: base.screen.map(value => value * k) as [number, number, number, number],
      faceBox: base.faceBox?.map(value => value * k) as [number, number, number, number] | undefined };
  }, [frame, k]);
  // On Bento's left side (and inside a fullscreen window's right edge) it peeks out leftwards.
  // Walking left, the side-on walk is mirrored (its images face right).
  const mirrored = dock === 'left' || dock === 'inside' || (walking && walkDir === -1);
  const shownMotion: Motion = greet ? 'none' : travel ? 'run' : motion;

  // Where the character sits inside the window.
  const placement = useMemo(() => {
    if (pose === 'peek') {
      return { left: mirrored ? WIN_W - WALL - spec.w : WALL, top: SIDE_Y };
    }
    if (pose === 'hang') {
      return { left: HOME_X - spec.w / 2, top: HANG_Y };
    }
    // In the corner the figure's right side stays put, so wide pastimes grow leftwards.
    // The body stays in one spot whatever it holds: poses line up on the face
    // screen's centre, so swapping poses never makes it jump sideways.
    // Every stool frame lines up on the plain seated pose, so the stool never shifts when the pose changes.
    const centre = stool ? (IMAGES.sit.screen[0] + IMAGES.sit.screen[2] / 2) * k : spec.screen[0] + spec.screen[2] / 2;
    // Wide pastimes (the sword) slide left just enough to stay inside the window.
    if (dock === 'corner') return { left: Math.max(8, Math.min(CORNER_X - centre, WIN_W - 8 - spec.w)), top: ledge - spec.ledge };
    return { left: HOME_X - centre, top: ledge - spec.ledge };
  }, [pose, mirrored, spec, ledge, dock, stool, k]);


  // Eyes follow the pointer (or wherever it is glancing).
  const look = useMemo<[number, number]>(() => {
    if (travel) return [travel.dir * 0.9, 0];
    if (greet) return [0, 0];
    if (motion === 'walk') return [walkDir * 0.8, 0];
    if (face === 'working') return [-0.25, 0.7];
    if (face === 'typing') return [0, 0.85];
    if (autoGaze) return autoGaze;
    const [sx, sy, sw, sh] = spec.screen;
    const cx = trackX.current + placement.left + (mirrored ? spec.w - sx - sw : sx) + sw / 2;
    const cy = placement.top + sy + sh / 2;
    const dx = gaze[0] - cx;
    const dy = gaze[1] - cy;
    const distance = Math.hypot(dx, dy);
    if (distance > 900) return [0, 0];
    const pull = distance / (distance + 90);
    return [dx / (distance || 1) * pull, dy / (distance || 1) * pull];
  }, [greet, travel, motion, walkDir, face, autoGaze, spec, placement, mirrored, gaze]);

  // ---- regions & input ----------------------------------------------------------
  pushRegions.current = () => {
    const boxes: number[][] = [];
    if (tucked) { void invoke('pet_set_regions', { regions: boxes }).catch(() => undefined); return; }
    const body = bodyRef.current?.getBoundingClientRect();
    if (body) boxes.push([body.left + 4, body.top + 2, body.width - 8, body.height - 4]);
    const box = bubbleRef.current?.getBoundingClientRect();
    if (bubble && box && box.width > 0) boxes.push([box.left - 2, box.top - 2, box.width + 4, box.height + 4]);
    void invoke('pet_set_regions', { regions: boxes }).catch(() => undefined);
  };
  useLayoutEffect(() => { pushRegions.current(); }, [bubble, frame, placement, tucked, typedAll]);

  // Clicking a bubble just brings Bento to the front; the user sees there what finished or waits.
  const openBento = () => {
    muteHover();
    setAnnouncing(undefined);
    void invoke('pet_action', { action: 'open', projectId: null, slot: null }).catch(() => undefined);
  };
  // Long-press (or press and move) to drag; works on Bento and in the corner.
  const holdTimer = useRef<number>();
  const startDrag = () => {
    const start = press.current;
    if (!start || start.dragged) return;
    start.dragged = true;
    window.clearTimeout(clickTimer.current);
    // Picked up: startled, dangling; a bit dizzy once it is put down.
    setReaction('surprised');
    setMotion('dangle');
    window.setTimeout(() => setReaction('dizzy'), 900);
    window.setTimeout(() => { setReaction(undefined); setMotion(current => current === 'dangle' ? 'none' : current); }, 2600);
    stopWalk();
    // In the corner pet.rs moves the window with the pointer itself (AppKit's window drag does not take
    // hold on that non-activating panel); on Bento the system drag carries it.
    void invoke('pet_stop').then(() => invoke('pet_drag_start')).then(() => ON_BENTO ? getCurrentWindow().startDragging() : undefined).catch(() => undefined);
  };
  const onPointerDown = (event: React.PointerEvent) => {
    // Pressing the corner pet must not bring Bento forward.
    if (!ON_BENTO) void invoke('pet_press').catch(() => undefined);
    if (event.button !== 0) return;
    press.current = { x: event.screenX, y: event.screenY, dragged: false };
    window.clearTimeout(holdTimer.current);
    holdTimer.current = window.setTimeout(startDrag, 320);
  };
  // Rubbing back and forth over its head is a head pat.
  const pat = useRef<{ x: number; dir: number; turns: number; at: number }>();
  const patted = useRef(0);
  const notePat = (event: React.PointerEvent) => {
    // Pats, like pokes, only in the corner; since round 10 during any pastime too.
    if (ON_BENTO || dock !== 'corner') { pat.current = undefined; return; }
    const box = bodyRef.current?.getBoundingClientRect();
    if (!box || event.clientY > box.top + box.height * 0.42) { pat.current = undefined; return; }
    const now = Date.now();
    const last = pat.current;
    if (!last || now - last.at > 600) { pat.current = { x: event.clientX, dir: 0, turns: 0, at: now }; return; }
    const dx = event.clientX - last.x;
    if (Math.abs(dx) < 3) return;
    const dir = Math.sign(dx);
    const turns = last.dir !== 0 && dir !== last.dir ? last.turns + 1 : last.turns;
    pat.current = { x: event.clientX, dir, turns, at: now };
    if (turns < 3 || now - patted.current < 2500) return;
    patted.current = now;
    pat.current = undefined;
    pokedAt.current = now;
    fondle(4);
    // Mid-pastime the pastime's own motion and prop come back afterwards.
    const keep = { motion: motionNow.current === 'pat' ? 'none' as Motion : motionNow.current, prop: propNow.current === 'hearts' ? undefined : propNow.current };
    setReaction(pick<Face>([['shy', 2], ['content', 2], ['love', 1]]));
    setMotion('pat');
    setProp('hearts');
    window.setTimeout(() => {
      setReaction(undefined);
      setProp(current => current === 'hearts' ? keep.prop : current);
      setMotion(current => current === 'pat' ? keep.motion : current);
    }, 1800);
  };
  const onPointerMove = (event: React.PointerEvent) => {
    if ((event.buttons & 1) === 0) notePat(event);
    const start = press.current;
    if (!start || start.dragged || (event.buttons & 1) === 0) return;
    if (Math.hypot(event.screenX - start.x, event.screenY - start.y) < 4) return;
    window.clearTimeout(holdTimer.current);
    startDrag();
  };
  const onPointerUp = () => window.clearTimeout(holdTimer.current);
  const openMenu = () => {
    setReaction('happy');
    window.setTimeout(() => setReaction(undefined), 1200);
    void invoke('pet_menu').catch(() => undefined);
  };
  // Corner pokes: a short reaction, then back to what it was doing. Poke it a lot and it gets dizzy.
  // Since round 10 a poke works during any pastime: the pastime keeps running underneath, and its
  // motion and prop come back when the reaction settles.
  const motionNow = useRef(motion);
  motionNow.current = motion;
  const propNow = useRef(prop);
  propNow.current = prop;
  const pokeKeep = useRef<{ motion: Motion; prop: typeof prop }>();
  const pokeTimers = useRef<number[]>([]);
  const pokes = useRef<number[]>([]);
  const lastPoke = useRef<string>();
  useEffect(() => () => { for (const id of pokeTimers.current) window.clearTimeout(id); }, []);
  const poke = () => {
    for (const id of pokeTimers.current) window.clearTimeout(id);
    pokeTimers.current = [];
    const later = (ms: number, fn: () => void) => { pokeTimers.current.push(window.setTimeout(fn, ms)); };
    const now = Date.now();
    pokedAt.current = now;
    pokes.current = [...pokes.current.filter(at => now - at < 3000), now];
    // What the pastime was doing before the first of a run of pokes.
    if (!pokeKeep.current) pokeKeep.current = { motion: motionNow.current, prop: propNow.current };
    const settle = (ms: number) => later(ms, () => {
      const keep = pokeKeep.current;
      pokeKeep.current = undefined;
      setReaction(undefined); setGreet(undefined);
      setProp(atBaseRef.current ? undefined : keep?.prop);
      setMotion(atBaseRef.current ? 'breathe' : keep?.motion ?? 'none');
    });
    setGreet(undefined);
    if (pokes.current.length >= 4) {
      pokes.current = [];
      setReaction('dizzy'); setProp(undefined); setMotion('tilt');
      later(1600, () => setReaction('pout'));
      settle(2600);
      return;
    }
    fondle(2);
    // How it takes a poke depends on how fond of us it is.
    const fond = affection.current;
    const weights: [string, number][] = fond >= 60
      ? [['giggle', 2], ['wave', 2], ['shy', 3], ['hop', 3], ['surprise', 1], ['pout', 0.5]]
      : fond < 25
        ? [['surprise', 3], ['pout', 2], ['giggle', 1], ['wave', 1], ['shy', 0.5], ['hop', 0.5]]
        : [['giggle', 2], ['surprise', 2], ['wave', 2], ['shy', 2], ['pout', 1], ['hop', 2]];
    let kind = pick(weights);
    if (kind === lastPoke.current) kind = pick(weights);
    lastPoke.current = kind;
    if (kind === 'giggle') { setReaction('happy'); setMotion('wiggle'); settle(1100); return; }
    if (kind === 'surprise') { setReaction('surprised'); setMotion('hop'); later(700, () => setReaction('happy')); settle(1500); return; }
    if (kind === 'shy') { setReaction('shy'); setProp('hearts'); setMotion('pat'); settle(1700); return; }
    if (kind === 'pout') { setReaction('pout'); setMotion('tilt'); later(1300, () => setReaction('smug')); settle(2200); return; }
    if (kind === 'hop') { setReaction('love'); setMotion('hop'); setSparkle(Date.now()); settle(1300); return; }
    // Waves back at us.
    setReaction('happy'); setMotion('none');
    let waves = 0;
    setGreet('wave-a');
    const swing = () => later(220, () => {
      waves += 1;
      if (waves >= 6) { setGreet(undefined); return; }
      setGreet(waves % 2 ? 'wave-b' : 'wave-a');
      swing();
    });
    swing();
    settle(6 * 220 + 300);
  };
  // A single click in the corner is a poke, whatever pastime it is in (round 10).
  // When something needs the user, a click opens the menu as before.
  const cornerClick = () => {
    if (!active || urgent) { openMenu(); return; }
    poke(); return;
    pokedAt.current = Date.now();
    setReaction('surprised');
    setResetKey(value => value + 1);
    window.setTimeout(() => setReaction(undefined), 700);
  };
  // Click and right-click open the pet's own menu. In the corner, a double
  // click brings Bento back instead, so a single click waits a beat to be sure.
  useEffect(() => () => { window.clearTimeout(clickTimer.current); window.clearTimeout(holdTimer.current); }, []);
  const onClick = (event: React.MouseEvent) => {
    const start = press.current;
    press.current = undefined;
    if (start?.dragged) return;
    if (ON_BENTO) { openMenu(); return; }
    window.clearTimeout(clickTimer.current);
    if (event.detail >= 2) {
      setReaction('happy');
      window.setTimeout(() => setReaction(undefined), 1200);
      openBento();
      return;
    }
    clickTimer.current = window.setTimeout(cornerClick, 260);
  };
  const onContextMenu = (event: React.MouseEvent) => { event.preventDefault(); openMenu(); };

  // Side-on (walking) the face sits on the narrow side screen, squeezed as if seen from the side.
  const [sx, sy, sw, sh] = spec.faceBox ?? spec.screen;
  const faceStyle = {
    left: mirrored ? spec.w - sx - sw : sx, top: sy, width: sw, height: sh,
    transform: spec.faceBox ? 'scaleX(.55)' : `rotate(${mirrored ? -spec.tilt : spec.tilt}deg)`,
    padding: spec.inset * Math.min(sw, sh),
  } as CSSProperties;
  const characterStyle = {
    left: placement.left, top: placement.top, width: spec.w, height: spec.h,
    '--walk-ms': `${walkMs}ms`, '--walk-dir': travel?.dir ?? walkDir,
    // Which way it tucks behind the window's edge: -1 = to its left, 1 = to its right.
    '--tuck': mirrored ? 1 : -1,
  } as CSSProperties;
  // The visor ring: spins while an AI CLI is thinking, glows amber while something waits for you,
  // flashes once when something finishes.
  const ring: 'busy' | 'wait' | 'done' | undefined = urgent ? 'wait' : (view?.thinking ?? 0) > 0 ? 'busy' : doneFlash ? 'done' : undefined;
  const ringPad = Math.min(sw, sh) * 0.09;
  const ringStyle = {
    left: (mirrored ? spec.w - sx - sw : sx) - ringPad, top: sy - ringPad, width: sw + ringPad * 2, height: sh + ringPad * 2,
    transform: `rotate(${mirrored ? -spec.tilt : spec.tilt}deg)`,
  } as CSSProperties;
  const ringEl = ring && !spec.faceBox && IMAGES[frame]?.face !== false && <svg className="pet-ring" data-ring={ring} style={ringStyle}
    viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
    <rect x="2" y="2" width="96" height="96" rx="34" ry="34" pathLength={100} vectorEffect="non-scaling-stroke" />
  </svg>;
  const faceEl = IMAGES[frame]?.face !== false && <PetFace face={face} look={look} blink={blink && !STROKE_EYES.has(face)} style={faceStyle} />;
  // Winter: the stool frames wear a red knit scarf (baked into their -w art, so it sits behind arms, mug and book).
  const winter = season();
  const frameSrc = `/pet/${IMAGES[frame]?.src ?? frame}.webp`;
  const bubbleSide = pose === 'peek' && !mirrored ? 'right' : 'left';
  const bubbleStyle: CSSProperties = bubbleSide === 'left'
    ? { right: WIN_W - placement.left + 4, top: Math.max(4, placement.top + spec.h * 0.12) }
    : { left: placement.left + spec.w + 6, top: placement.top + 6 };

  return <main className="pet-root" data-pet data-dock={dock} data-active={active || undefined} data-tucked={tucked || undefined}
    style={{ '--pet-opacity': style.opacity / 100 } as CSSProperties} onContextMenu={onContextMenu}>
    {dock === 'corner' && winter.snow && active && <span className="pet-snow" aria-hidden="true"><i /><i /><i /><i /><i /><i /><i /></span>}
    <div ref={trackRef} className="pet-track">
    {onLedge && !away && pose !== 'peek' && pose !== 'hang' && <div className="pet-shadow" data-pose={shownPose}
      style={{ left: placement.left + spec.w * 0.14, width: spec.w * 0.72, top: ledge - 4, height: 8 * k + 2 }} />}
    <div ref={bodyRef} className="pet-character" data-pose={shownPose} data-motion={shownMotion} data-landing={landing || undefined} data-stool={stool || undefined}
      data-mirrored={mirrored || undefined} data-away={away || undefined} style={characterStyle} role="button" aria-label="Bento 桌宠"
      onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onClick={onClick}
      onPointerEnter={() => { if (!hoverMuted.current) setHovering(true); }}>
      <div className="pet-figure">
        {stool ? <>
          {/* On the stool the body breathes on its own: the stool and feet below the seat line stay put. */}
          <img className="pet-legs" src={frameSrc} alt="" draggable={false}
            style={{ clipPath: `inset(${SEAT * 100}% 0 0 0)` }} />
          <div className="pet-upper" style={{ clipPath: `inset(-70% -40% ${(1 - SEAT) * 100 - 0.8}% -40%)`, transformOrigin: `50% ${SEAT * 100}%` }}>
            <img ref={imgRef} src={frameSrc} alt="" draggable={false} style={{ transformOrigin: '50% 100%' }} />
            {frame === 'book' && pageTurn && <PageTurn key={pageTurn} src={frameSrc} />}
            {ringEl}
            {faceEl}
          </div>
        </> : <>
          {walking
            ? <div className="pet-walk-sheet" aria-hidden="true" style={{
              backgroundImage: `url(${walkSheet(shownPose as 'walk' | 'walk-hp')})`,
              backgroundPosition: `${(stride % WALK_POSES) * 100 / (WALK_POSES - 1)}% 0`,
            }} />
            : <img ref={imgRef} src={`/pet/${IMAGES[frame]?.src ?? frame}.webp`} alt="" draggable={false} style={{ transformOrigin: '50% 100%' }} />}
          {prop === 'blade' && frame === 'sword-spin' && <span className="pet-blade" aria-hidden="true"
            style={{ '--fx': `${FIST[0] * k}px`, '--fy': `${FIST[1] * k}px`, '--k': k } as CSSProperties}>
            <svg className="pet-blade-trail" viewBox="-100 -50 200 100" aria-hidden="true">
              <path d="M0 0 C 30 -46, 92 -46, 92 0 S 30 46, 0 0 S -92 -46, -92 0 S -30 46, 0 0 Z" pathLength={100} />
            </svg>
            <span className="pet-blade-spin"><img src="/pet/c-blade.webp" alt="" draggable={false} /></span>
          </span>}
          {ringEl}
          {faceEl}
        </>}
      </div>
      {face === 'sleep' && <span className="pet-z" aria-hidden="true">z</span>}
      {motion === 'bob' && <span className="pet-notes" aria-hidden="true"><b>♪</b><b>♫</b></span>}
      {sparkle && <span key={sparkle} className="pet-sparkles" aria-hidden="true"><b>✦</b><b>✦</b><b>✧</b><b>✦</b></span>}
      {prop === 'puff' && <span className="pet-puff" aria-hidden="true" />}
      {prop === 'hearts' && <span className="pet-hearts" aria-hidden="true"><b>♥</b><b>♥</b><b>♥</b></span>}
      {(prop === 'steam' || prop === 'sigh') && <svg className="pet-steam" data-sigh={prop === 'sigh' || undefined} viewBox="0 0 20 30"
        style={{ left: (spec.screen[0] + spec.screen[2] / 2) - 8 * k, top: spec.ledge - (pose === 'mug-sip' ? (stool ? 84.2 : 80) : (stool ? 67.2 : 58)) * k, width: 16 * k }} aria-hidden="true">
        <path d="M6 28 C 2 22, 10 18, 6 12 S 8 4, 6 0" /><path d="M13 28 C 9 22, 17 18, 13 12 S 15 4, 13 0" />
      </svg>}
      {prop === 'zz' && <span className="pet-zz" style={{ left: spec.w * 0.74, top: spec.h * 0.04 }} aria-hidden="true"><b>z</b><b>z</b><b>Z</b></span>}
      {prop === 'bubbles' && <span className="pet-bubbles" style={{ left: spec.w * 0.417, top: spec.h * 0.351 }} aria-hidden="true">{[0, 1, 2, 3, 4].map(index => <i key={index} />)}</span>}
      {prop === 'plane' && <svg className="pet-plane" viewBox="0 0 24 14" style={{ left: spec.w * 0.096 - 10 * k, top: spec.h * 0.379 - 12 * k, width: 22 * k }} aria-hidden="true">
        <path d="M23 1 L1 6 L8 8 Z" fill="#fff" /><path d="M23 1 L8 8 L10 13 Z" fill="#e4e1db" /><path d="M23 1 L8 8" stroke="#cfcac2" strokeWidth=".6" />
      </svg>}
      {/* The scarf pastime's snowflake: drifts down and settles on the raised mitten. */}
      {prop === 'flake' && <svg className="pet-flake" viewBox="-10 -10 20 20" style={{ left: spec.w * 0.092 - 7 * k, top: spec.h * 0.31 - 14 * k, width: 14 * k, height: 14 * k, '--drop': `${-46 * k}px` } as CSSProperties} aria-hidden="true">
        <g stroke="#fff" strokeWidth="1.6" strokeLinecap="round">{[0, 60, 120].map(angle => <g key={angle} transform={`rotate(${angle})`}><path d="M0 -8 V8" /><path d="M-2.4 -5.6 L0 -3.6 L2.4 -5.6 M-2.4 5.6 L0 3.6 L2.4 5.6" /></g>)}</g>
      </svg>}
      {prop === 'code' && <span className="pet-code" style={{ left: (spec.screen[0] + spec.screen[2] / 2) - 30 * k, top: spec.ledge - (stool ? 57.1 : 62) * k }} aria-hidden="true">
        <b>{'{ }'}</b><b>;</b><b>{'</>'}</b><b>=&gt;</b>
      </span>}
      {(prop === 'sing' || prop === 'pour') && <span className="pet-notes pet-sing" style={{ left: (spec.screen[0] + spec.screen[2] / 2) + (stool ? 50 : 36) * k, top: spec.ledge - (stool ? 127.1 : 128) * k }} aria-hidden="true"><b>♪</b><b>♫</b><b>♪</b></span>}
      {/* The pour frame tips the can so its spout sits right over the pot; the drops fall onto its leaves. */}
      {pose === 'water-pour' && <span className="pet-water" style={{ left: spec.w * 0.229 - 3, top: spec.h * 0.69, '--fall': `${spec.h * 0.12}px` } as CSSProperties} aria-hidden="true"><i /><i /><i /><i /><i /></span>}
      {prop === 'petals' && <span className="pet-petals" aria-hidden="true"><b /><b /><b /><b /><b /></span>}
      {prop === 'slash' && <span className="pet-slash" aria-hidden="true" />}
      {face === 'think' && <span className="pet-think" aria-hidden="true"><b /><b /><b><i /></b></span>}
      {prop === 'fish' && <svg className="pet-fish" aria-hidden="true" width="40" height={Math.max(40, WIN_H_PX - (placement.top + spec.h * 0.62))}
        style={{ left: spec.w * 0.86, top: spec.h * 0.62 }}>
        <path d={`M2 0 Q 18 6 22 18 L 22 ${Math.max(40, WIN_H_PX - (placement.top + spec.h * 0.62)) - 14}`} />
        <circle className="pet-bobber" cx="22" cy={Math.max(40, WIN_H_PX - (placement.top + spec.h * 0.62)) - 10} r="3.2" />
      </svg>}
      {face === 'alert' && <span className="pet-mark" aria-hidden="true">!</span>}
    </div>
    <div ref={bubbleRef} className="pet-bubble" data-side={bubbleSide} data-tone={bubble?.tone}
      data-visible={!!bubble || undefined} style={bubbleStyle} onClick={() => { if (!bubble) return; if (bubble.key.startsWith('rest-')) setRest(undefined); else openBento(); }}>
      {bubble && <span key={bubble.key + bubble.text} className="pet-bubble-text">
        {bubble.tone !== 'tip' && <i aria-hidden="true" />}{bubble.text.slice(0, typed)}
        {!typedAll && <span className="pet-caret" aria-hidden="true" />}
      </span>}
      {bubble && bubble.tone !== 'tip' && <span className="pet-bubble-actions" data-shown={typedAll || undefined}>
        <button type="button" onClick={event => { event.stopPropagation(); if (bubble.item) openItem(bubble.item); else openBento(); }}>打开</button>
        <button type="button" onClick={event => { event.stopPropagation(); putOff(bubble); }}>稍后</button>
      </span>}
    </div>
    </div>
  </main>;
}

// ---- turning a page -------------------------------------------------------------
// Drawn over c-book-flat (437 x 467 px at 3x), in that image's pixels, so it scales and breathes with the body.
// The right page's sheet is lifted by the right mitten, rises, stands up over the spine (bending as paper does)
// and settles onto the left page, where it fades into it.
const PAGE_TURN_MS = 1500;
type Pt = [number, number];
const BOOK = {
  spineTop: [222, 283] as Pt, spineBottom: [224, 303] as Pt,
  rightTop: [302, 266] as Pt, rightBottom: [306, 284] as Pt,
  leftTop: [133, 275] as Pt, leftBottom: [128, 292] as Pt,
  // How tall the sheet looks standing straight up over the spine.
  up: 74,
};
const ease = (t: number) => t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2;
const clamp01 = (t: number) => Math.max(0, Math.min(1, t));
// Where an edge of the sheet is when it has turned `angle` (0 = lying on the right page, PI = on the left one).
const sheetEdge = (from: Pt, right: Pt, left: Pt, angle: number, length = 1): Pt => {
  const flat = Math.cos(angle);
  const side = flat >= 0 ? right : left;
  const reach = Math.abs(flat) * length;
  return [from[0] + (side[0] - from[0]) * reach, from[1] + (side[1] - from[1]) * reach - BOOK.up * Math.sin(angle) * length];
};
function PageTurn({ src }: { src: string }) {
  const [t, setT] = useState(0);
  useEffect(() => {
    if (reducedMotion()) { setT(1); return; }
    const start = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      const value = Math.min(1, (now - start) / PAGE_TURN_MS);
      setT(value);
      if (value < 1) raf = window.requestAnimationFrame(tick);
    };
    raf = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(raf);
  }, []);
  // 0-20%: the mitten pinches and lifts the corner; 20-77%: the sheet turns over; then it lies on the left and fades.
  const angle = t < 0.2 ? 0.22 * ease(t / 0.2) : 0.22 + (Math.PI - 0.22) * ease(clamp01((t - 0.2) / 0.57));
  // The part of the sheet near the spine leads and the free edge trails behind, so the sheet bends.
  const lead = Math.min(Math.PI, angle + 0.45 * Math.sin(angle));
  const { spineTop: st, spineBottom: sb, rightTop: rt, rightBottom: rb, leftTop: lt, leftBottom: lb } = BOOK;
  const edgeTop = sheetEdge(st, rt, lt, angle);
  const edgeBottom = sheetEdge(sb, rb, lb, angle);
  const bendTop = sheetEdge(st, rt, lt, lead, 0.55);
  const bendBottom = sheetEdge(sb, rb, lb, lead, 0.55);
  const p = (pt: Pt) => `${pt[0].toFixed(1)} ${pt[1].toFixed(1)}`;
  const d = `M${p(st)} Q${p(bendTop)} ${p(edgeTop)} L${p(edgeBottom)} Q${p(bendBottom)} ${p(sb)} Z`;
  const back = angle > Math.PI / 2;
  const shade = 0.78 + 0.22 * Math.abs(Math.cos(angle));
  const fade = t < 0.86 ? 1 : 1 - clamp01((t - 0.86) / 0.12);
  // The right mitten: lifts with the corner, follows it a little way, then lets go and settles back.
  const lift = t < 0.2 ? ease(t / 0.2) : t < 0.42 ? 1 : 1 - ease(clamp01((t - 0.42) / 0.2));
  const shadow = Math.sin(angle) * (1 - clamp01((t - 0.7) / 0.15));
  return <>
    <svg className="pet-page-turn" viewBox="0 0 437 467" preserveAspectRatio="none" aria-hidden="true">
      <defs>
        <linearGradient id="pet-sheet" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor={back ? '#e9e2d3' : '#fbf8f1'} />
          <stop offset="1" stopColor={back ? '#f7f2e8' : '#ece5d6'} />
        </linearGradient>
      </defs>
      {/* The lifted sheet's shadow on the page underneath. */}
      <path d={`M${p(st)} L${p(rt)} L${p(rb)} L${p(sb)} Z`} fill="#000" opacity={0.16 * shadow} />
      <g opacity={fade}>
        <path d={d} fill="url(#pet-sheet)" stroke="#bdb3a1" strokeWidth={1.1} strokeLinejoin="round" />
        {/* Turned edge-on to the light it darkens a little. */}
        <path d={d} fill="#5a4a32" opacity={(1 - shade) * 0.9} />
        {/* A few lines of text, faint, so it reads as a printed page. */}
        <path d={`M${p(sheetEdge(st, rt, lt, angle, 0.2))} L${p(sheetEdge(st, rt, lt, angle, 0.85))}`} stroke="#cfc6b6" strokeWidth={1.4} opacity={0.6} />
      </g>
    </svg>
    <img className="pet-page-hand" src={src} alt="" draggable={false} style={{
      transform: `translate(${(-1.2 * lift).toFixed(2)}%, ${(-1.6 * lift).toFixed(2)}%) rotate(${(-7 * lift).toFixed(2)}deg)`,
    }} />
  </>;
}

// Faces whose eyes are drawn closed or as symbols: no blinking for them.
const STROKE_EYES = new Set<Face>(['happy', 'sleep', 'yawn', 'error', 'squint', 'content', 'shy', 'cool', 'smug', 'dizzy', 'angry', 'love', 'music']);
const FACE_COLOR: Partial<Record<Face, string>> = { error: '#ff8a7a', angry: '#ff8a7a', alert: '#ffd27a', love: '#ff9ec4', shy: '#ff9ec4' };

// The visor face: soft, rounded vector marks in mint (capsule eyes with a catch-light, curved mouths,
// a warm blush on the happy faces) with a gentle glow under the glass highlight. Drawn in a 100 x 80 box.
const EYE_GAP = 17.5;
const EYE_Y = 30;
const MOUTH_Y = 55;
const BLUSH_FACES = new Set<Face>(['idle', 'happy', 'content', 'shy', 'love', 'music', 'curious']);
const arc = (x: number, y: number, w: number, h: number) => `M${x - w / 2} ${y} Q${x} ${y + h} ${x + w / 2} ${y}`;
const heart = (x: number, y: number, r: number) =>
  `M${x} ${y + r * 0.9} C${x - r * 1.6} ${y - r * 0.1} ${x - r * 0.9} ${y - r * 1.3} ${x} ${y - r * 0.45} C${x + r * 0.9} ${y - r * 1.3} ${x + r * 1.6} ${y - r * 0.1} ${x} ${y + r * 0.9}Z`;

/** One eye, centred on (x, y). side: -1 left, 1 right (for marks that mirror). */
function Eye({ face, x, y, side }: { face: Face; x: number; y: number; side: -1 | 1 }) {
  const stroke = { fill: 'none', stroke: 'currentColor', strokeWidth: 3.6, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const };
  const capsule = (w: number, h: number, light = true) => <>
    <rect x={x - w / 2} y={y - h / 2} width={w} height={h} rx={w / 2} />
    {light && <circle cx={x + w * 0.2} cy={y - h * 0.24} r={Math.min(w, h) * 0.19} fill="#fff" fillOpacity={0.9} />}
  </>;
  switch (face) {
    case 'happy': return <path d={arc(x, y + 4, 14, -12)} {...stroke} />;
    case 'content': case 'shy': case 'music': return <path d={arc(x, y - 1, 13, 6)} {...stroke} />;
    case 'sleep': case 'yawn': return <path d={arc(x, y + 1, 13, 5)} {...stroke} strokeWidth={3} />;
    case 'cool': return <rect x={x - 7.5} y={y - 2} width={15} height={4} rx={2} />;
    case 'smug': return side < 0 ? <rect x={x - 7} y={y - 1.8} width={14} height={3.6} rx={1.8} /> : <path d={arc(x, y + 3, 13, -9)} {...stroke} />;
    case 'squint': return <path d={`M${x - 5 * side} ${y - 6} L${x + 5 * side} ${y} L${x - 5 * side} ${y + 6}`} {...stroke} />;
    case 'error': return <path d={`M${x - 5} ${y - 5} L${x + 5} ${y + 5} M${x + 5} ${y - 5} L${x - 5} ${y + 5}`} {...stroke} />;
    case 'surprised': return <circle cx={x} cy={y} r={6.5} {...stroke} strokeWidth={3.2} />;
    case 'love': return <path d={heart(x, y, 6.5)} />;
    case 'dizzy': return <path d={`M${x} ${y} m0 -1.5 a1.5 1.5 0 1 1 -1.5 1.5 a3.5 3.5 0 1 1 3.5 3.5 a5.5 5.5 0 1 1 -5.5 -5.5`} {...stroke} strokeWidth={2.6} />;
    case 'angry': return <>
      {capsule(8, 12, false)}
      <path d={`M${x - 7 * side} ${y - 11} L${x + 6 * side} ${y - 7}`} {...stroke} strokeWidth={3} />
    </>;
    case 'pout': return capsule(7.5, 11);
    case 'alert': case 'curious': return capsule(10.5, 19);
    case 'typing': return <path d={`M${x - 6.5} ${y + 1} Q${x} ${y + 6} ${x + 6.5} ${y + 1} L${x + 6.5} ${y + 3.5} Q${x} ${y + 9.5} ${x - 6.5} ${y + 3.5}Z`} />;
    default: return capsule(9, 16);
  }
}

function PetFace({ face, look, blink, style }: { face: Face; look: [number, number]; blink: boolean; style: CSSProperties }) {
  const color = FACE_COLOR[face] ?? '#8ff5d0';
  const blushId = `pet-blush-${useId().replace(/:/g, '')}`;
  const still = face === 'sleep' || face === 'yawn' || face === 'content' || face === 'shy' || face === 'squint' || face === 'dizzy' || face === 'music';
  const [lx, ly] = still ? [0, 0] : look;
  // The eyes glide toward where it looks; thinking glances up and to the side.
  const [dx, dy] = face === 'think' ? [3.5, -4] : [lx * 3, ly * 2.4];
  const mx = 50 + (face === 'think' ? 4 : lx * 1.4);
  const my = MOUTH_Y + ly;
  const stroke = { fill: 'none', stroke: 'currentColor', strokeWidth: 3.2, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const };

  const mouth = (() => {
    switch (face) {
      case 'happy': return <path d={`M${mx - 7} ${my - 1} Q${mx} ${my + 11} ${mx + 7} ${my - 1}Z`} />;
      case 'content': case 'love': case 'music': case 'idle': case 'curious': return <path d={arc(mx, my, 13, 6)} {...stroke} />;
      case 'shy': return <path d={`M${mx - 6} ${my + 1} q1.5 -2.5 3 0 t3 0 t3 0 t3 0`} {...stroke} strokeWidth={2.4} />;
      case 'cool': case 'smug': return <path d={`M${mx - 5} ${my + 1} Q${mx + 2} ${my + 4} ${mx + 7} ${my - 2}`} {...stroke} />;
      case 'think': return <rect x={mx - 4} y={my - 1.6} width={9} height={3.2} rx={1.6} />;
      case 'surprised': case 'alert': return <ellipse cx={mx} cy={my + 1} rx={3.4} ry={4} {...stroke} strokeWidth={2.8} />;
      case 'yawn': return <ellipse cx={mx} cy={my + 1} rx={4.5} ry={6} />;
      case 'dizzy': case 'error': return <path d={`M${mx - 8} ${my + 1} q2 -3 4 0 t4 0 t4 0 t4 0`} {...stroke} strokeWidth={2.6} />;
      case 'pout': case 'angry': return <path d={arc(mx, my + 3, 11, -5)} {...stroke} />;
      case 'sleep': return <rect x={mx - 4} y={my - 1.5} width={8} height={3} rx={1.5} />;
      case 'working': return <g className="pet-dots">{[0, 1, 2].map(index =>
        <circle key={index} cx={mx - 6 + index * 6} cy={my} r={2} />)}</g>;
      case 'typing': return <g>
        {/* A rounded ">" prompt with keys lighting up as it types. */}
        <path d={`M${mx - 9} ${my - 4} L${mx - 5} ${my} L${mx - 9} ${my + 4}`} {...stroke} strokeWidth={2.8} />
        <g className="pet-keys">{[0, 1, 2].map(index =>
          <rect key={index} x={mx - 2 + index * 4.5} y={my + 2} width={3.4} height={2.8} rx={1.2} />)}</g>
      </g>;
      default: return <path d={arc(mx, my, 13, 6)} {...stroke} />;
    }
  })();

  const eyeStyle: CSSProperties = { transform: `translate(${dx}px, ${dy}px)`, transition: 'transform .22s cubic-bezier(.3, .7, .4, 1)' };
  const blinkStyle: CSSProperties = { transformBox: 'fill-box', transformOrigin: 'center', transform: blink ? 'scaleY(.12)' : 'none', transition: 'transform .07s ease-in-out' };
  return <svg className="pet-face" viewBox="0 0 100 80" preserveAspectRatio="xMidYMid meet" style={{ ...style, color }} aria-hidden="true">
    <defs>
      <radialGradient id={blushId}>
        <stop offset="0" stopColor="#ff8fa3" stopOpacity={0.6} />
        <stop offset="1" stopColor="#ff8fa3" stopOpacity={0} />
      </radialGradient>
    </defs>
    {BLUSH_FACES.has(face) && <g className="pet-blush">
      <ellipse cx={50 - EYE_GAP - 7} cy={MOUTH_Y - 6} rx={8} ry={4.5} fill={`url(#${blushId})`} opacity={face === 'shy' || face === 'love' ? 1 : 0.75} />
      <ellipse cx={50 + EYE_GAP + 7} cy={MOUTH_Y - 6} rx={8} ry={4.5} fill={`url(#${blushId})`} opacity={face === 'shy' || face === 'love' ? 1 : 0.75} />
    </g>}
    <g fill="currentColor">
      <g style={eyeStyle}>
        <g className="pet-eye" style={blinkStyle}><Eye face={face} x={50 - EYE_GAP} y={EYE_Y} side={-1} /></g>
        <g className="pet-eye" style={blinkStyle}><Eye face={face} x={50 + EYE_GAP} y={EYE_Y} side={1} /></g>
      </g>
      {mouth}
    </g>
    {/* Glass: a soft highlight across the upper left, over the marks. */}
    <path className="pet-face-glass" fill="#fff" fillOpacity={0.14} d="M8 22 C 14 8, 30 3, 48 4 C 34 7, 20 14, 13 28 Z" />
  </svg>;
}
