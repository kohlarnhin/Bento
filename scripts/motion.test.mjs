import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  advanceProjectViewTime, blendProjectViews, dashboardEntranceLimit, invertMotionBox, motionSeconds, projectViewBlendSeconds,
  projectViewContentOpacities, projectViewContentTiming, projectViewOpacities, projectViewOrder, readyProjectView, REDUCED_MOTION_QUERY,
} from '../src/lib/motionMath.ts';

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

test('Pending terminal paint or Git data preserves the current view in both motion modes', () => {
  assert.equal(readyProjectView('terminal', 'overview', false, true), 'overview');
  assert.equal(readyProjectView('terminal', 'git', false, true), 'git');
  assert.equal(readyProjectView('terminal', 'git', true, true), 'terminal');
  assert.equal(readyProjectView('git', 'terminal', true, false), 'terminal');
  assert.equal(readyProjectView('git', 'terminal', true, true), 'git');
  assert.equal(readyProjectView('overview', 'terminal', false, false), 'overview');
  assert.equal(motionSeconds(projectViewBlendSeconds, true), 0);
  assert.ok(projectViewBlendSeconds >= .16 && projectViewBlendSeconds <= .22);
});

const initialView = name => Object.fromEntries(projectViewOrder.map(view => [view, Number(view === name)]));
const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-12, a + ' != ' + b);

test('A late frame limits the visual step without changing the regular 200 ms blend', () => {
  near(advanceProjectViewTime(0, .08), .024);
  let elapsed = 0;
  for (let frame = 0; frame < 12; frame++) elapsed = advanceProjectViewTime(elapsed, 1 / 60);
  near(elapsed, projectViewBlendSeconds);
  assert.equal(advanceProjectViewTime(elapsed, .02), projectViewBlendSeconds);
});

function verifyComposition(mix) {
  const alpha = projectViewOpacities(mix);
  near(alpha.overview * (1 - alpha.git) * (1 - alpha.terminal), mix.overview);
  near(alpha.git * (1 - alpha.terminal), mix.git);
  near(alpha.terminal, mix.terminal);
  near((1 - alpha.overview) * (1 - alpha.git) * (1 - alpha.terminal), 0);
}

test('Every pair of workspace views blends opaque surfaces with no exposed backing frame', () => {
  for (const from of projectViewOrder) {
    for (const to of projectViewOrder) {
      const initial = initialView(from);
      for (const progress of [0, .08, .35, .5, .92, 1]) {
        const mix = blendProjectViews(initial, to, progress);
        verifyComposition(mix);
        if (from !== to) near(mix[to], progress);
      }
      assert.deepEqual(blendProjectViews(initial, to, 1), initialView(to));
    }
  }
});

test('An interrupted transition keeps the last painted frame across a redirect or reversal', () => {
  const current = blendProjectViews(initialView('terminal'), 'git', .36);
  for (const target of projectViewOrder) {
    assert.deepEqual(blendProjectViews(current, target, 0), current);
    verifyComposition(blendProjectViews(current, target, .24));
    verifyComposition(blendProjectViews(current, target, .65));
    assert.deepEqual(blendProjectViews(current, target, 1), initialView(target));
  }
});

test('The workspace foregrounds overlap for 30 ms without a frame with neither visible', () => {
  const durationMs = projectViewBlendSeconds * 1000;
  for (const from of projectViewOrder) {
    for (const to of projectViewOrder.filter(view => view !== from)) {
      const initial = initialView(from);
      for (let ms = 0; ms <= durationMs; ms++) {
        const content = projectViewContentOpacities(initial, to, ms / 1000);
        assert.ok(content[from] + content[to] > 0);
        assert.equal(content[from] > 0 && content[to] > 0,
          ms > projectViewContentTiming.enter * 1000 && ms < projectViewContentTiming.exit * 1000);
      }
      assert.deepEqual(projectViewContentOpacities(initial, to, projectViewBlendSeconds), initialView(to));
    }
  }
});

test('Redirecting a foreground blend starts with the exact current foregrounds', () => {
  const current = projectViewContentOpacities(initialView('overview'), 'terminal', .072);
  assert.ok(current.overview > 0 && current.terminal > 0);
  assert.deepEqual(projectViewContentOpacities(current, 'git', 0), current);
  assert.deepEqual(projectViewContentOpacities(current, 'git', .2), initialView('git'));
});
