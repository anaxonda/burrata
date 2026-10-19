import { describe, expect, it } from 'vitest';
import { webviewAudioCapabilities } from './webview-audio-capabilities';

describe('WebView codec capabilities', () => {
  it('reports Vorbis and Opus independently from the actual media implementation', () => {
    expect(webviewAudioCapabilities({canPlayType: mime => mime.includes('opus') ? 'probably' : ''})).toEqual({oggVorbis:false,oggOpus:true});
    expect(webviewAudioCapabilities({canPlayType: () => 'maybe'})).toEqual({oggVorbis:true,oggOpus:true});
  });
  it('treats unavailable codec queries as unsupported without failing startup', () => {
    expect(webviewAudioCapabilities({canPlayType: () => {throw new Error('unavailable');}})).toEqual({oggVorbis:false,oggOpus:false});
  });
});
