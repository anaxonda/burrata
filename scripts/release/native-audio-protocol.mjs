/** A clock is supporting evidence; consumed stereo PCM is required. */
function hasValidPositionAndSeek(sample) {
  return Number.isFinite(sample?.position) && sample.position >= 0
    && Number.isSafeInteger(sample?.seekSerial) && sample.seekSerial >= 0;
}
export function assessNativeOutput(sample, trackId) {
  if (sample?.engine !== 'native' || sample.trackId !== trackId || !Number.isSafeInteger(sample.generation)
      || !Number.isFinite(sample.capturedSeconds) || sample.capturedSeconds < 2.9
      || !Number.isSafeInteger(sample.sampleRate) || sample.sampleRate < 8000 || sample.sampleRate > 0xffff_ffff
      || !hasValidPositionAndSeek(sample) || sample.channels !== 2 || sample.measurements?.length !== 2) {
    throw new Error('Native output did not capture the complete stereo fixture');
  }
  const [left, right] = sample.measurements;
  if (![left, right].every(channel => Number.isFinite(channel.rms) && channel.rms > .025 && channel.rms < .25)
      || !(left.tone440 > .06 && left.tone660 < left.tone440 * .05)
      || !(right.tone660 > .05 && right.tone440 < right.tone660 * .05)) {
    throw new Error('Native output fixture tones or stereo lanes did not match');
  }
  return {engine:'native',channels:2,sampleRate:sample.sampleRate,capturedSeconds:sample.capturedSeconds,
    measurements:sample.measurements,evidence:sample.evidence};
}
export function assessNativeProgress(first, second) {
  if (!hasValidPositionAndSeek(first) || !hasValidPositionAndSeek(second)
      || first.generation !== second.generation || first.trackId !== second.trackId
      || first.seekSerial !== second.seekSerial || second.position - first.position < .5) {
    throw new Error('Native output ownership changed or consumption stalled');
  }
}

/** Only an actual frontend oneshot layout response can establish mounted UI. */
export function assessFrontendLayout(response) {
  const layout = response?.structuredContent?.result ?? response?.structuredContent;
  const footer = Array.isArray(layout?.elements) && layout.elements.find(item => item.role === 'footer');
  if (response?.isError === true || !Number.isFinite(layout?.viewport?.width) || layout.viewport.width <= 0
      || !Number.isFinite(layout?.viewport?.height) || layout.viewport.height <= 0
      || footer?.present !== true || footer.visible !== true
      || !Number.isFinite(footer.width) || footer.width <= 0 || !Number.isFinite(footer.height) || footer.height <= 0) {
    throw new Error('Actual frontend layout response is missing or not rendered');
  }
  return {frontendMounted:true, frontendProof:'frontend_ready plus real layout oneshot', viewport:layout.viewport};
}

/** Fail closed before volume1: this must be the owned app on the owned virtual route. */
export function assessPrivateLinuxRoute(config, facts) {
  const fail = () => { throw new Error('Owned private Linux output route is not verified; remain muted'); };
  if (facts?.platform !== 'linux' || config?.private !== true || !Number.isSafeInteger(config.uid)
      || config.uid < 0 || typeof config.appExe !== 'string' || !config.appExe.startsWith('/')
      || typeof config.pulse?.exe !== 'string' || !config.pulse.exe.startsWith('/')
      || !/^\d+$/.test(config.pulse?.startTicks ?? '')
      || typeof config.tempRoot !== 'string' || !config.tempRoot.startsWith('/')
      || config.socket !== `${config.tempRoot}/pulse-runtime/native` || typeof config.socket !== 'string' || !config.socket.startsWith('/')
      || facts.envServer !== `unix:${config.socket}` || facts.envSink !== 'burrata_smoke'
      || facts.socket?.isSocket !== true || facts.socket.uid !== config.uid
      || facts.socket.canonical !== true || facts.socket.parentMode !== 0o700
      || facts.pulse?.pid !== config.pulse?.pid || facts.pulse?.exe !== config.pulse?.exe
      || facts.pulse?.startTicks !== config.pulse?.startTicks || facts.pulse?.uid !== config.uid
      || !Number.isSafeInteger(facts.pulse?.pid) || facts.pulse.pid <= 0 || !['R','S','D','I'].includes(facts.pulse?.state)
      || !Number.isSafeInteger(config.appPgid) || config.appPgid <= 0) fail();
  const modules = facts.modules;
  if (!Array.isArray(modules) || modules.length !== 2) fail();
  const nulls = modules.filter(m => m.name === 'module-null-sink');
  const native = modules.filter(m => m.name === 'module-native-protocol-unix');
  if (nulls.length !== 1 || native.length !== 1
      || ![nulls[0].index,native[0].index].every(n => Number.isSafeInteger(n) && n >= 0)) fail();
  const args = text => new Set(String(text ?? '').split(/\s+/));
  if (!['sink_name=burrata_smoke','rate=44100','channels=2','norewinds=1'].every(x => args(nulls[0].argument).has(x))
      || !args(native[0].argument).has(`socket=${config.socket}`)) fail();
  if (!Array.isArray(facts.sinks) || facts.sinks.length !== 1) fail();
  const sink = facts.sinks[0];
  if (!Number.isSafeInteger(sink.index) || sink.index < 0 || sink.name !== 'burrata_smoke' || sink.driver !== 'module-null-sink.c'
      || sink.owner_module !== nulls[0].index || sink.monitor_source_name !== 'burrata_smoke.monitor'
      || !/\b2ch\b.*\b44100Hz\b/.test(sink.sample_specification ?? '')) fail();
  if (!Array.isArray(facts.inputs) || facts.inputs.length !== 1) fail();
  const input = facts.inputs[0], app = facts.app;
  if (!Number.isSafeInteger(input.sink) || input.sink < 0 || input.sink !== sink.index || !Number.isSafeInteger(app?.pid) || app.pid <= 0
      || String(input.properties?.['application.process.id']) !== String(app.pid)
      || app.pgid !== config.appPgid || app.exe !== config.appExe || app.uid !== config.uid || !['R','S','D','I'].includes(app.state)) fail();
  return {verified:true, isolatedVirtualOutput:true, ownedAppStream:true, physicalHardware:false};
}

export async function unmuteVerifiedPrivateOutput(config, facts, call) {
  const proof = assessPrivateLinuxRoute(config, facts);
  await call('set_volume', {volume:1});
  return proof;
}

// One shared deadline across backend readiness and actual frontend response.
export function remainingCheckTimeout(deadline, now=Date.now(), cap=7000) {
  const remaining=deadline-now;
  if(!Number.isFinite(remaining)||remaining<=0)throw new Error('Installed MCP check budget exhausted');
  return Math.max(1,Math.min(cap,remaining));
}
export async function verifyRemuteWhilePlaying(before,trackId,waitQuiet,inspect,pause) {
  await waitQuiet();
  const after=await inspect();
  assessNativeOutput(after,trackId);
  assessNativeProgress(before,after);
  await pause();
  return {remuteWhilePlaying:true,remuteClockAdvanceSeconds:after.position-before.position};
}
