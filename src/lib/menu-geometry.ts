export interface MenuAnchorRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface MenuSize {
  width: number;
  height: number;
}

export interface MenuViewport {
  width: number;
  height: number;
}

export interface MenuPosition {
  left: number;
  top: number;
  maxHeight: number;
}

export function positionContextMenu(
  anchor: MenuAnchorRect,
  menu: MenuSize,
  viewport: MenuViewport,
  playerHeight: number,
  gutter = 8,
): MenuPosition {
  const right = Math.max(gutter, viewport.width - gutter);
  const bottom = Math.max(gutter, viewport.height - Math.max(0, playerHeight) - gutter);
  const maxHeight = Math.max(0, bottom - gutter);
  const width = Math.min(menu.width || 200, Math.max(0, viewport.width - gutter * 2));
  const height = Math.min(menu.height || 240, maxHeight);
  const left = Math.max(gutter, Math.min(anchor.right - width, right - width));
  const roomBelow = bottom - anchor.bottom - gutter;
  const roomAbove = anchor.top - gutter;
  const proposedTop = height <= roomBelow || roomBelow >= roomAbove
    ? anchor.bottom + gutter
    : anchor.top - height - gutter;
  const top = Math.max(gutter, Math.min(proposedTop, bottom - height));
  return { left, top, maxHeight };
}
