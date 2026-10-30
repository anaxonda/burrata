import { describe, expect, it, vi } from 'vitest';
import { webviewAudioCapabilities, createWebviewAudioCapabilitiesProbe } from './webview-audio-capabilities';

describe('WebView codec capabilities', () => {
  it('reports Vorbis and Opus independently from the actual media implementation', () => {
    expect(webviewAudioCapabilities({canPlayType: mime => mime.includes('opus') ? 'probably' : ''})).toEqual({oggVorbis:false,oggOpus:true});
    expect(webviewAudioCapabilities({canPlayType: () => 'maybe'})).toEqual({oggVorbis:true,oggOpus:true});
  });
  it('treats unavailable codec queries as unsupported without failing startup', () => {
    expect(webviewAudioCapabilities({canPlayType: () => {throw new Error('unavailable');}})).toEqual({oggVorbis:false,oggOpus:false});
  });
});

it('initializes media capabilities once per WebView probe and reuses the result', () => {
  const canPlayType = vi.fn().mockReturnValue('probably');
  vi.stubGlobal('document', { createElement: vi.fn(() => ({ canPlayType })) });
  try {
    const probe = createWebviewAudioCapabilitiesProbe();
    expect(probe()).toEqual({ oggVorbis: true, oggOpus: true });
    expect(probe()).toBe(probe());
    expect(canPlayType).toHaveBeenCalledTimes(2);
  } finally { vi.unstubAllGlobals(); }
});
