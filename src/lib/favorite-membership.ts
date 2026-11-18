import type { Track } from "./types";

export const FAVORITE_MEMBERSHIP = Symbol("favorite membership");
export type FavoriteMembership = (track: Track) => boolean;
