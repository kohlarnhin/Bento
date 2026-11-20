export function sidebarThumb(viewportHeight: number, scrollHeight: number, scrollTop: number) {
  if (viewportHeight <= 0 || scrollHeight <= viewportHeight) return;
  const height = Math.min(viewportHeight, Math.max(32, viewportHeight ** 2 / scrollHeight));
  const travel = viewportHeight - height;
  const scrollRange = scrollHeight - viewportHeight;
  const position = Math.min(Math.max(0, scrollTop), scrollRange) / scrollRange;
  return { height, top: position * travel, scrollPerPixel: travel ? scrollRange / travel : 0 };
}

function menuAxis(anchor: number, size: number, viewport: number) {
  const edge = 8;
  const lastStart = viewport - size - edge;
  const start = anchor > lastStart ? anchor - size : anchor;
  return Math.max(edge, Math.min(start, lastStart));
}

export function sidebarContextPosition(x: number, y: number, width: number, height: number, viewportWidth: number, viewportHeight: number) {
  return { left: menuAxis(x, width, viewportWidth), top: menuAxis(y, height, viewportHeight) };
}
