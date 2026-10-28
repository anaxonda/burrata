import { describe, it, expect } from "vitest";
import { territoryLabel } from "./territory-label";
describe("territory labels", () => {
  it("expands MusicBrainz special territories and standard country codes", () => {
    expect(territoryLabel("XW")).toBe("Worldwide");
    expect(territoryLabel("xe")).toBe("Europe");
    expect(territoryLabel("US")).toBe("United States");
    expect(territoryLabel("GB")).toBe("United Kingdom");
  });
  it("keeps full names and handles missing metadata", () => {
    expect(territoryLabel("France")).toBe("France");
    expect(territoryLabel(null)).toBe("");
    expect(territoryLabel(undefined)).toBe("");
  });
});
