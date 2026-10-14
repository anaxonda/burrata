import { describe, expect, it } from "vitest";
import { rowWindow } from "./table";
describe("virtual table navigation", () => {
  it("keeps a filtered list visible after scrolling a much larger list", () => {
    const window = rowWindow(1100, 170000, 600);
    expect(window.start).toBeLessThan(1100);
    expect(window.end).toBe(1100);
    expect(window.end - window.start).toBeGreaterThan(0);
  });
  it("retains overscan around a keyboard destination without rendering thousands of rows", () => {
    const window = rowWindow(5000, 120000, 600);
    expect(window.start).toBeLessThanOrEqual(2000);
    expect(window.end).toBeGreaterThan(2000);
    expect(window.end - window.start).toBeLessThan(50);
  });
  it("renders all rows of small and empty charts", () => {
    expect(rowWindow(100, 0, 600)).toEqual({ start: 0, end: 100 });
    expect(rowWindow(0, 120000, 600)).toEqual({ start: 0, end: 0 });
  });
});
