import { describe, expect, it } from "vitest";
import { positionContextMenu } from "./menu-geometry";

describe("positionContextMenu", () => {
  it.each([90, 130])("keeps an oversized menu above a %i px player footer", (playerHeight) => {
    const viewport = { width: 707, height: 480 };
    const position = positionContextMenu(
      { left: 680, right: 700, top: 455, bottom: 475 },
      { width: 240, height: 410 },
      viewport,
      playerHeight,
    );
    const renderedHeight = Math.min(410, position.maxHeight);
    expect(position.left).toBeGreaterThanOrEqual(8);
    expect(position.left + 240).toBeLessThanOrEqual(viewport.width - 8);
    expect(position.top).toBeGreaterThanOrEqual(8);
    expect(position.top + renderedHeight).toBeLessThanOrEqual(viewport.height - playerHeight - 8);
    expect(position.maxHeight).toBe(viewport.height - playerHeight - 16);
  });

  it("prefers the side with more space and clamps short menus to the viewport", () => {
    const viewport = { width: 707, height: 480 };
    const anchor = { left: 650, right: 690, top: 220, bottom: 250 };
    const position = positionContextMenu(anchor, { width: 220, height: 120 }, viewport, 90);
    expect(position.left).toBe(470);
    expect(position.top).toBe(258);
  });
});
