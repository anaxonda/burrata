import {describe, expect, it} from 'vitest';
import {artistPlaybackTracks} from './artist-playback';
import type {LocalAsset, Track} from './types';
const track = {id:'catalog:1',artist:'Gillian Welch',title:'Caleb Meyer',sourceUrl:'https://musicbrainz.org/recording/source',version:'',durationSeconds:200,provenance:{sourceKind:'catalog'}} as Track;
const asset = {id:'asset:1',path:'/Music/song.flac',track:{...track,id:'local:1',localAvailability:'available'}} as LocalAsset;
describe('artist playback audio',()=>{
  it('reuses available local audio while preserving catalog identity and source metadata',()=>{
    const result=artistPlaybackTracks([track],[asset])[0];
    expect(result.localPath).toBe(asset.path);expect(result.id).toBe(track.id);expect(result.sourceUrl).toBe(track.sourceUrl);
  });
  it('does not choose an ambiguous copy, different mix, duration mismatch or missing file',()=>{
    for(const assets of [[asset,asset],[{...asset,track:{...asset.track,version:'Live'}}],[{...asset,track:{...asset.track,durationSeconds:300}}],[{...asset,track:{...asset.track,localAvailability:'missing'}}]]){
      expect(artistPlaybackTracks([track],assets)[0].localPath).toBeUndefined();
    }
  });
});
