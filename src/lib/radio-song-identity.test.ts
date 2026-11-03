import { describe, expect, it } from "vitest";
import { radioSongTrackIdentity } from "./radio-song-identity";

describe("radio song track identity", () => {
  it("projects a canonical listing ID into matching storage identity fields", () => {
    const id = `radio_song:${"a".repeat(64)}`;
    expect(radioSongTrackIdentity(id)).toEqual({
      id,
      provider: "radio_song",
      providerId: "a".repeat(64),
    });
  });

  it.each(["", "radio_song:", "radio_song:   ", "radio:recording", "radio_song:radio_song:abc"])(
    "rejects malformed listing ID %s",
    (id) => {
      expect(radioSongTrackIdentity(id)).toBeNull();
    },
  );
});
