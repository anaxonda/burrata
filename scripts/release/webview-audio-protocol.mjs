export function webviewMediaProbeConfirmsEngine(playbackState, mediaInspection) {
  const stateEngine = playbackState?.engine;
  return (stateEngine === undefined || stateEngine === 'webview')
    && mediaInspection?.engine === 'webview'
    && mediaInspection?.applicable === true;
}
