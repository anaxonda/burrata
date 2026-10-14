const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function harness(autoReady = true) {
  const nodes = new Map(), requests = [], players = [], widgets = [], clock={now:1000};
  function element() { return {textContent:'',children:[],appendChild(el){this.children.push(el);if(el.onload)queueMicrotask(el.onload);},replaceChildren(){this.children=[];}}; }
  class Player {
    constructor(id,options){this.options=options;this.destroyed=false;this.calls=[];players.push(this);if(autoReady)queueMicrotask(()=>options.events.onReady({target:this}));}
    destroy(){this.destroyed=true;} playVideo(){this.calls.push('play');}pauseVideo(){this.calls.push('pause');}
    seekTo(value){this.calls.push(['seek',value]);}setVolume(value){this.calls.push(['volume',value]);}
    getPlayerState(){return 1;}getCurrentTime(){return 12;}getDuration(){return 180;}
  }
  const events = Object.fromEntries(['READY','PLAY','PAUSE','FINISH','PLAY_PROGRESS','ERROR'].map(k=>[k,k]));
  function Widget(frame) {
    const widget={frame,handlers:{},calls:[],bind(name,cb){this.handlers[name]=cb;},getDuration(cb){cb(200000);},
      play(){this.calls.push('play');},pause(){this.calls.push('pause');},seekTo(value){this.calls.push(['seek',value]);},setVolume(value){this.calls.push(['volume',value]);}};
    widgets.push(widget);queueMicrotask(()=>widget.handlers.READY?.());return widget;
  }
  Widget.Events=events;
  const sandbox={URL,Promise,console,encodeURIComponent,Date:{now:()=>clock.now},setTimeout:()=>1,clearTimeout:()=>{},clearInterval:()=>{},setInterval:()=>1,
    location:{href:'http://127.0.0.1:8888/player?token=secret',origin:'http://127.0.0.1:8888'},
    document:{getElementById(id){if(!nodes.has(id))nodes.set(id,element());return nodes.get(id);},createElement:element,head:element()},
    YT:{Player},SC:{Widget},fetch:async(path,options)=>{if(path==='/player/session')return {ok:false};requests.push({path,options,body:JSON.parse(options.body)});return {ok:true};}
  };
  sandbox.window=sandbox; vm.runInNewContext(fs.readFileSync(`${__dirname}/player.js`,'utf8'),sandbox);
  return {command:sandbox.burrataCandidateCommand,nodes,requests,players,widgets,clock};
}
const source=id=>({action:'start',sessionId:id,sourceId:id,provider:'youtube',title:'A <script> tag is plain text',mediaId:'M7lc1UVf-VE',url:'https://youtube.com/watch?v=M7lc1UVf-VE'});
const settle=()=>new Promise(resolve=>setImmediate(resolve));

test('YouTube attempts playback on readiness with visible official controls and authenticated status',async()=>{
  const h=harness();await h.command(source('one'));await settle();
  assert.deepEqual(h.players[0].calls,['play']);
  assert.equal(h.players[0].options.height,270);assert.equal(h.players[0].options.playerVars.autoplay,0);
  assert.equal(h.players[0].options.playerVars.controls,1);assert.equal(h.players[0].options.playerVars.origin,'http://127.0.0.1:8888');
  assert.equal(h.nodes.get('title').textContent,'A <script> tag is plain text');
  assert.equal(h.requests[0].options.headers.Authorization,'Bearer secret');
  h.command({action:'play',sessionId:'one'});h.command({action:'seek',sessionId:'one',value:25});h.command({action:'volume',sessionId:'one',value:0.4});
  assert.deepEqual(h.players[0].calls,['play','play',['seek',25],['volume',40]]);
});
test('switching destroys previous player and ignores stale controls/status callbacks',async()=>{
  const h=harness();await h.command(source('one'));await settle();const previous=h.players[0];
  await h.command(source('two'));await settle();assert.equal(previous.destroyed,true);
  const count=h.requests.length;previous.options.events.onStateChange({data:1});assert.equal(h.requests.length,count);
  const calls=h.players[1].calls.length;h.command({action:'play',sessionId:'one'});assert.equal(h.players[1].calls.length,calls);
  h.command({action:'stop'});assert.equal(h.players[1].destroyed,true);
});
test('a stale YouTube readiness callback does not auto-play its old session',async()=>{
  const h=harness(false);await h.command(source('one'));const previous=h.players[0];
  await h.command(source('two'));const current=h.players[1];
  previous.options.events.onReady({target:previous});
  assert.equal(previous.destroyed,true);assert.deepEqual(previous.calls,[]);
  current.options.events.onReady({target:current});assert.deepEqual(current.calls,['play']);
});
test('YouTube autoplay blocking reports a playable fallback',async()=>{
  const h=harness();await h.command(source('blocked'));await settle();
  h.players[0].options.events.onAutoplayBlocked();
  assert.match(h.nodes.get('message').textContent,/Autoplay was blocked.*Press Play below/);
  assert.match(h.requests.at(-1).body.error,/Autoplay was blocked/);
});
test('SoundCloud widget translates seconds to milliseconds and reports playback',async()=>{
  const h=harness();await h.command({...source('sc'),provider:'soundcloud',url:'https://soundcloud.com/artist/song'});await settle();
  const widget=h.widgets[0];assert.match(widget.frame.src,/auto_play=false/);assert.deepEqual(widget.calls,['play']);
  h.command({action:'seek',sessionId:'sc',value:25});assert.deepEqual(widget.calls,['play',['seek',25000]]);
  widget.handlers.PLAY();widget.handlers.PLAY_PROGRESS({currentPosition:12000});
  const latest=h.requests.at(-1).body;assert.equal(latest.paused,false);assert.equal(latest.position,12);assert.equal(latest.duration,200);
  const beforeProgress=h.requests.length;
  for(let i=0;i<50;i++)widget.handlers.PLAY_PROGRESS({currentPosition:13000+i});
  assert.equal(h.requests.length,beforeProgress,'progress reports limited to one per 500ms');
  widget.handlers.PAUSE();assert.equal(h.requests.length,beforeProgress+1,'pause reports immediately');
  h.clock.now+=500;widget.handlers.PLAY_PROGRESS({currentPosition:14000});
  assert.equal(h.requests.length,beforeProgress+2);assert.equal(h.requests.at(-1).body.position,14);
  widget.handlers.PLAY();assert.equal(h.requests.length,beforeProgress+3,'play reports immediately');
  widget.handlers.ERROR();assert.equal(h.requests.length,beforeProgress+4,'errors report immediately');
  assert.match(h.requests.at(-1).body.error,/unavailable/);
  await h.command(source('yt'));await settle();const count=h.requests.length;
  widget.handlers.PLAY_PROGRESS({currentPosition:30000});assert.equal(h.requests.length,count);
  assert.ok(widget.calls.includes('pause'));
});
