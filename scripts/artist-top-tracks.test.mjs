import test from 'node:test';
import assert from 'node:assert/strict';
import {executeJob} from './provider-session-worker.mjs';
const artistId='0123456789ABCDEFGHIJKL',listUri=`spotify:list:popular-release-segments-main-roles:artist_${artistId}`;
const trackId=n=>String(n+1).padStart(22,'0');
function row(n){return {uid:`row-${n}`,entity:{__typename:'EntityResponseWrapper',_uri:`spotify:track:${trackId(n)}`,data:{__typename:'Entity',entityTypeTrait:{__typename:'EntityTypeTrait',type:'ENTITY_TYPE_TRACK'},identityTrait:{__typename:'IdentityTrait',name:`Song ${n}`,contributors:{items:[{name:'Fixture Artist',uri:`spotify:artist:${artistId}`}]},contentHierarchyParent:{uri:'spotify:album:abcdefghijklmnopqrstuv',identityTrait:{__typename:'IdentityTrait',name:'Fixture Album'}}},consumptionExperienceTrait:{__typename:'ConsumptionExperienceTrait',duration:{seconds:'121',nanoSeconds:250000000}},visualIdentityTrait:{__typename:'VisualIdentityTrait',squareCoverImage:{image:{data:{__typename:'ImageV2',sources:[{url:'https://i.scdn.co/image/fixture',maxWidth:640,maxHeight:640}]}}}}}}};}
async function run(args={},change=x=>x){const calls=[];const result=await executeJob({provider:'spotify',operation:'getArtistTracks',args:{artistId,...args},cookieHeader:'sp_dc=fixture-not-a-real-cookie',cachedAuth:{accessToken:'fixture-token',expiresAt:Date.now()+60000}},{fetchImpl:async(url,options={})=>{
 const target=new URL(url).href;
 if(target==='https://francescograzioso.github.io/Meld/spotify-gql-hashes.json')return Response.json({operations:{}});
 assert.equal(target,'https://api-partner.spotify.com/pathfinder/v2/query');const body=JSON.parse(options.body);calls.push(body);
 if(body.operationName==='profileAttributes')return Response.json({data:{me:{profile:{uri:'spotify:user:fixture-account'}}}});
 assert.equal(body.operationName,'getListsContents');assert.equal(body.extensions.persistedQuery.sha256Hash,'130589ef0ac5ffc096b08ab11c031292812f91620665af1a339112ca3016c5c1');
 assert.deepEqual(body.variables,{uris:[listUri],offset:Number(args.cursor??0),limit:30});
 const start=body.variables.offset,rows=Array.from({length:Math.min(30,Math.max(0,31-start))},(_,i)=>row(start+i));
 return Response.json(change({data:{lists:[{__typename:'List',items:{totalCount:31,items:rows}}]}}));
 }});return{result,calls};}
test('source-pinned artist list pages preserve metadata, fractional duration and duplicate recording occurrences',async()=>{
 const first=await run();assert.equal(first.result.ok,true,JSON.stringify(first.result));const page=first.result.result;
 assert.equal(page.artistId,artistId);assert.equal(page.items.length,30);assert.equal(page.nextCursor,'30');assert.equal(page.total,31);
 assert.equal(page.items[0].durationSeconds,121.25);assert.equal(page.items[0].artist,'Fixture Artist');assert.equal(page.items[0].album,'Fixture Album');assert.equal(page.items[0].artworkUrl,'https://i.scdn.co/image/fixture');assert.equal(Object.hasOwn(page.items[0],'plays'),false);
 const next=await run({cursor:page.nextCursor},body=>{body.data.lists[0].items.items[0]=row(0);return body;});assert.equal(next.result.ok,true);assert.equal(next.result.result.items[0].trackId,page.items[0].trackId);assert.equal(next.result.result.items[0].position,31);assert.equal(next.result.result.nextCursor,null);
});
test('artist rows preserve parent album identity and only explicit release positions as recording context',async()=>{
 const {result}=await run({},body=>{const data=body.data.lists[0].items.items[0].entity.data;data.trackNumber=2;data.discNumber=1;data.externalIds={isrc:'USABC2600001'};data.identityTrait.contentHierarchyParent.date={isoString:'1988-05-03'};return body;});
 assert.equal(result.ok,true,JSON.stringify(result));const item=result.result.items[0];assert.equal(item.position,1);assert.equal(JSON.stringify(item.recordingContext),JSON.stringify({albumId:'abcdefghijklmnopqrstuv',albumUri:'spotify:album:abcdefghijklmnopqrstuv',trackNumber:2,discNumber:1,isrc:'USABC2600001',releaseDate:'1988-05-03'}));
 const legacy=await run();assert.equal(legacy.result.ok,true);const context=legacy.result.result.items[0].recordingContext;assert.equal(JSON.stringify(context),JSON.stringify({albumId:'abcdefghijklmnopqrstuv',albumUri:'spotify:album:abcdefghijklmnopqrstuv'}));assert.equal(Object.hasOwn(context,'trackNumber'),false);assert.equal(Object.hasOwn(context,'discNumber'),false);assert.equal(Object.hasOwn(context,'isrc'),false);
 const unrelatedParent=await run({},body=>{const parent=body.data.lists[0].items.items[0].entity.data.identityTrait.contentHierarchyParent;parent.uri='spotify:artist:0123456789ABCDEFGHIJKL';parent.identityTrait.name='Unrelated parent';parent.date={isoString:'1988-05-03'};return body;});assert.equal(unrelatedParent.result.ok,true,JSON.stringify(unrelatedParent.result));assert.equal(unrelatedParent.result.result.items[0].album,'');assert.equal(Object.hasOwn(unrelatedParent.result.result.items[0],'recordingContext'),false);
 const conflicting=await run({},body=>{body.data.lists[0].items.items[0].entity.data.identityTrait.contentHierarchyParent._uri='spotify:album:ZZZZZZZZZZZZZZZZZZZZZZ';return body;});assert.equal(conflicting.result.ok,false);assert.equal(conflicting.result.error.code,'PROVIDER_CHANGED');
});

test('unknown totals and duration values stay unknown; nanosecond-only durations are preserved',async()=>{
 const {result}=await run({},body=>{const list=body.data.lists[0].items;list.totalCount=null;list.items[0].entity.data.consumptionExperienceTrait.duration=null;list.items[1].entity.data.consumptionExperienceTrait.duration={nanoSeconds:200000000};list.items[2].entity.data.consumptionExperienceTrait.duration={seconds:1,nanoSeconds:1000000000};list.items[3].entity.data.consumptionExperienceTrait.duration={seconds:true,nanoSeconds:0};return body;});
 assert.equal(result.ok,true);assert.equal(result.result.total,null);assert.equal(result.result.nextCursor,'30');assert.equal(result.result.items[0].durationSeconds,null);assert.equal(result.result.items[1].durationSeconds,.2);assert.equal(result.result.items[2].durationSeconds,null);assert.equal(result.result.items[3].durationSeconds,null);
});
test('wrong list identities, non-track entities and oversized pages are rejected',async()=>{
 for(const modify of [body=>{body.data.lists[0].uri=null;return body;},body=>{body.data.lists[0].uri='spotify:list:wrong';return body;},body=>{body.data.lists[0].items.items[0].entity.data.uri='spotify:track:zzzzzzzzzzzzzzzzzzzzzz';return body;},body=>{body.data.lists[0].items.totalCount=1;return body;},body=>{body.data.lists[0].items.items[0].entity.data.entityTypeTrait.type='ENTITY_TYPE_PODCAST_EPISODE';return body;},body=>{body.data.lists[0].items.items.push(row(31));return body;}]){const {result}=await run({},modify);assert.equal(result.ok,false);assert.equal(result.error.code,'PROVIDER_CHANGED');}
});
test('malformed artist ids and cursors never issue an artist list request',async()=>{
 for(const args of [{artistId:'short'},{cursor:'-1'},{cursor:'01'},{cursor:'100001'},{cursor:'https://example.com'}]){const {result,calls}=await run(args);assert.equal(result.ok,false);assert.equal(calls.some(call=>call.operationName==='getListsContents'),false);}
});

test('artist list inspection returns only fixed guard counts for actual worker responses',async()=>{
 const {result}=await run({inspectResponseShape:true});assert.equal(result.ok,true,JSON.stringify(result));
 const shape=result.result.responseShape;assert.equal(shape.firstFailedGuard,'none');assert.equal(shape.listIdentityMatches,false);assert.equal(shape.rowCount,30);assert.equal(shape.rows.validTrackUri,30);assert.equal(shape.rows.validContributors,30);assert.equal(shape.truncated,false);
 assert.equal(JSON.stringify(shape).includes(artistId),false);assert.equal(JSON.stringify(shape).includes('Fixture Artist'),false);assert.equal(JSON.stringify(shape).includes('spotify:'),false);
});
test('artist inspection pinpoints mismatched guards without echoing hostile GraphQL values',async()=>{
 for(const [modify,guard]of [
  [body=>{body.data.lists[0].uri='spotify:list:private-hostile';return body;},'list_identity'],
  [body=>{body.data.lists[0].items.items[0].entity.data.entityTypeTrait.type='HOSTILE_TYPE';return body;},'row_entity'],
  [body=>{delete body.data.lists[0].items.items[0].entity.data.identityTrait.contributors;return body;},'row_identity'],
  [body=>{body.data.lists[0].items.items[0].entity.data.identityTrait.contributors.items[0].uri='private-hostile';return body;},'row_contributors'],
  [body=>({...body,errors:[{message:'fixture-token private-hostile',extensions:{code:'PRIVATE_HOSTILE'}}]}),'graphql_errors']
 ]){const {result}=await run({inspectResponseShape:true},modify);assert.equal(result.ok,true,JSON.stringify(result));assert.equal(result.result.responseShape.firstFailedGuard,guard);assert.equal(JSON.stringify(result.result.responseShape).includes('hostile'),false);assert.equal(JSON.stringify(result.result.responseShape).includes('fixture-token'),false);}
});

test('artist list projection binds a singleton response to its request without requiring a URI echo',async()=>{
 const missing=await run();assert.equal(missing.result.ok,true);assert.equal(missing.result.result.items.length,30);
 const echoed=await run({},body=>{body.data.lists[0].uri=listUri;return body;});assert.equal(echoed.result.ok,true);
 const multiple=await run({},body=>{body.data.lists.push(body.data.lists[0]);return body;});assert.equal(multiple.result.ok,false);
 const malformed=await run({},body=>{body.data.lists[0].__typename='NotFound';return body;});assert.equal(malformed.result.ok,false);
});
