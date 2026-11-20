import assert from 'node:assert/strict';
import { test } from 'node:test';
import gsapPackage from 'gsap';
import { animateTerminalTab, dashboardEntranceLimit, invertMotionBox, motionSeconds, REDUCED_MOTION_QUERY, settledTerminalTab } from '../src/lib/motionMath.ts';

const gsap = gsapPackage.gsap ?? gsapPackage;

test('Reduced motion makes an animation settle immediately', () => {
  assert.equal(REDUCED_MOTION_QUERY, '(prefers-reduced-motion: reduce)');
  for (const seconds of [.12, .18, .2, .22, .24, .26, .32, .42]) {
    assert.equal(motionSeconds(seconds, false), seconds);
    assert.equal(motionSeconds(seconds, true), 0);
  }
});

test('FLIP starts at the previous painted position and size in both axes', () => {
  const previous = { x: 10, y: 75, width: 52, height: 36 };
  const next = { x: 82, y: 37, width: 104, height: 30 };
  assert.deepEqual(invertMotionBox(previous, next), { x: -72, y: 38, scaleX: .5, scaleY: 1.2 });
  assert.equal(invertMotionBox({ ...previous, width: 0 }, next), undefined);
  assert.equal(invertMotionBox(previous, { ...next, height: 0 }), undefined);
});

test('Overview stagger is capped at eight elements', () => {
  assert.equal(dashboardEntranceLimit, 8);
});

test('The terminal and heading only appear after the dark layer is opaque', t => {
  const paint = settledTerminalTab(false);
  const timeline = gsap.timeline({ paused: true });
  t.after(() => timeline.kill());
  animateTerminalTab(timeline, paint, true);

  timeline.time(.08);
  assert.ok(paint.backdrop > 0 && paint.backdrop < 1);
  assert.equal(paint.content, 0);
  assert.equal(paint.y, 4);

  timeline.time(.12);
  assert.deepEqual({ backdrop: paint.backdrop, content: paint.content, y: paint.y }, { backdrop: 1, content: 0, y: 4 });
  timeline.time(.2);
  assert.equal(paint.backdrop, 1);
  assert.ok(paint.content > 0 && paint.content < 1 && paint.y > 0 && paint.y < 4);
  assert.equal(timeline.duration(), .28);
  timeline.progress(1);
  assert.equal(paint.content, 1);
  assert.equal(paint.y, 0);
});

test('Leaving the terminal hides the content before dimming the layer', t => {
  const paint = settledTerminalTab(true);
  const timeline = gsap.timeline({ paused: true });
  t.after(() => timeline.kill());
  animateTerminalTab(timeline, paint, false);

  timeline.time(.08);
  assert.equal(paint.backdrop, 1);
  assert.ok(paint.content > 0 && paint.content < 1);
  timeline.time(.1);
  assert.equal(paint.backdrop, 1);
  assert.equal(paint.content, 0);
  timeline.time(.16);
  assert.ok(paint.backdrop > 0 && paint.backdrop < 1);
  assert.equal(paint.content, 0);
  assert.equal(timeline.duration(), .22);
  timeline.progress(1);
  assert.equal(paint.backdrop, 0);
});

test('Retargeting during either phase resumes from the painted values', t => {
  for (const [entering, points] of [[true, [.04, .16]], [false, [.04, .16]]]) {
    for (const interruption of points) {
      const paint = settledTerminalTab(!entering);
      const first = gsap.timeline({ paused: true });
      const reversed = gsap.timeline({ paused: true });
      t.after(() => { first.kill(); reversed.kill(); });
      animateTerminalTab(first, paint, entering);
      first.time(interruption);
      const lastFrame = { backdrop: paint.backdrop, content: paint.content };
      animateTerminalTab(reversed, paint, !entering);
      assert.deepEqual({ backdrop: paint.backdrop, content: paint.content }, lastFrame);
      for (const progress of [.1, .4, .7, 1]) {
        reversed.progress(progress);
        assert.ok(paint.backdrop === 1 || paint.content === 0);
      }
      const final = settledTerminalTab(!entering);
      assert.equal(paint.backdrop, final.backdrop);
      assert.equal(paint.content, final.content);
      if (!entering) assert.equal(paint.y, final.y);
    }
  }
});
