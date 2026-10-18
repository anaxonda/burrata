import {describe, expect, it} from 'vitest';
import {artistPlaybackTracks} from './artist-playback';
import fixture from './artist-playback-conformance.json';
import type {LocalAsset, Track} from './types';

describe('artist playback audio',()=>{
  for (const testCase of fixture.cases) {
    it(testCase.name,()=>{
      const source = testCase.sourceTrack as Track;
      const assets = testCase.assets as LocalAsset[];
      const result = artistPlaybackTracks([source], assets)[0];
      const expected = testCase.expectedAssetId
        ? assets.find(asset => asset.id === testCase.expectedAssetId)?.path
        : undefined;
      expect(result.localPath).toBe(expected);
      expect(result.id).toBe(source.id);
      expect(result.sourceUrl).toBe(source.sourceUrl);
    });
  }
});
