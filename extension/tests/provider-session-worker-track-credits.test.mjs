import test from 'node:test';
import assert from 'node:assert/strict';
import { executeJob, makeFetcher } from '../../scripts/provider-session-worker.mjs';

const TRACK_ID='2u9k9X8d1uD4qDLjMhVxU0';
const ARTIST_IDS=['3eDT9fwXKuHWFvgZaaYC5v','0NErdIJtuKBjtxKmlqaWdj','7HCqGPJcQTyGJ2yqntbuyr','5HcunTidTUrOaf8V0iJcvl','3GdSQUH1BRtl9UrrtuwJlP'];
const TITLE='Halka Halka (From "Fanney Khan")';
// Sanitized projection from the successful exact-track Spotify credits read.
// The live Track union contained no uri/title/duration echo and contributors
// exposed no count/pageInfo. Values below preserve the provider's ten role rows.
const liveRows=[
 {name:'Sunidhi Chauhan',uri:`spotify:artist:${ARTIST_IDS[0]}`,role:'Main Artist',roleGroup:{name:'Artist'}},
 {name:'Divya Kumar',uri:`spotify:artist:${ARTIST_IDS[1]}`,role:'Main Artist',roleGroup:{name:'Artist'}},
 {name:'Amit Trivedi',uri:`spotify:artist:${ARTIST_IDS[2]}`,role:'Main Artist',roleGroup:{name:'Artist'}},
 {name:'Nusrat Fateh Ali Khan',uri:`spotify:artist:${ARTIST_IDS[3]}`,role:'Main Artist',roleGroup:{name:'Artist'}},
 {name:'Irshad Kamil',uri:`spotify:artist:${ARTIST_IDS[4]}`,role:'Main Artist',roleGroup:{name:'Artist'}},
 {name:'Amit Trivedi',uri:`spotify:artist:${ARTIST_IDS[2]}`,role:'Composer',roleGroup:{name:'Composition & Lyrics'}},
 {name:'Nusrat Fateh Ali Khan',uri:`spotify:artist:${ARTIST_IDS[3]}`,role:'Composer',roleGroup:{name:'Composition & Lyrics'}},
 {name:'Irshad Kamil',uri:`spotify:artist:${ARTIST_IDS[4]}`,role:'Lyricist',roleGroup:{name:'Composition & Lyrics'}},
 {name:'Sunidhi Chauhan',uri:`spotify:artist:${ARTIST_IDS[0]}`,role:'Vocalist',roleGroup:{name:'Performers'}},
 {name:'Divya Kumar',uri:`spotify:artist:${ARTIST_IDS[1]}`,role:'Vocalist',roleGroup:{name:'Performers'}},
];
function fixtureFetch({trackType='Track',contributors=liveRows,includeTotal=false,totalCount=contributors.length,trackEcho=null,ignoreOffset=false}={}){
 const calls=[];
 const fetchImpl=async(url,options={})=>{
  const target=new URL(url).href;calls.push({target,options});
  if(target==='https://francescograzioso.github.io/Meld/spotify-gql-hashes.json')return Response.json({operations:{}});
  if(target==='https://api-partner.spotify.com/pathfinder/v2/query'){
   const request=JSON.parse(options.body);
   if(request.operationName==='profileAttributes')return Response.json({data:{me:{profile:{uri:'spotify:user:fixture-account',name:'Fixture'}}}});
   if(request.operationName==='queryNpvArtist'){
    const {contributorsLimit,contributorsOffset}=request.variables;
    return Response.json({data:{trackUnion:{__typename:trackType,associationsV3:null,canvas:null,creditsTrait:{contributors:{items:contributors.slice(ignoreOffset?0:contributorsOffset,(ignoreOffset?0:contributorsOffset)+contributorsLimit),...(includeTotal?{totalCount}:{})},sources:[]},merch:null,...trackEcho}}});
   }
   throw new Error(`unexpected operation ${request.operationName}`);
  }
  throw new Error(`unexpected URL ${target}`);
 };
 return {fetchImpl,calls};
}
function request(args={}){return {provider:'spotify',operation:'getTrackCredits',args:{trackId:TRACK_ID,trackTitle:TITLE,mainArtistId:ARTIST_IDS[0],durationMilliseconds:247381,artistIds:ARTIST_IDS,...args},cookieHeader:'sp_dc=fixture; sp_key=fixture',cachedAuth:{accessToken:'test-fixture-credential-a1b2c3d4e5',expiresAt:Date.now()+60000}};}

test('live exact-track shape stays bound to request URI and keeps role groups distinct',async()=>{
 const fixture=fixtureFetch();
 const result=await executeJob(request(),{fetchImpl:fixture.fetchImpl});
 assert.equal(result.ok,true,JSON.stringify(result));
 const value=result.result.spotifyTrackCredits;
 assert.equal(JSON.stringify(value.binding),JSON.stringify({trackId:TRACK_ID,trackUri:`spotify:track:${TRACK_ID}`,trackTitle:TITLE,expectedDurationMilliseconds:247381,artistIds:ARTIST_IDS,mainArtistUri:`spotify:artist:${ARTIST_IDS[0]}`,bindsThrough:'exact queryNpvArtist trackUri variable',providerEchoedTrackIdentity:false}));
 assert.equal(value.credits.rowCount,10);
 assert.equal(value.credits.paginationStatus,'terminal');
 assert.equal(value.credits.paginationTerminal,true);
 assert.equal(value.credits.pagesFetched,2);
 assert.equal(value.credits.roleEvidenceComplete,false);
 assert.equal(value.credits.performerCreditsComplete,false);
 assert.equal(value.credits.classificationReady,false);
 assert.equal(JSON.stringify(value.credits.performerPreview.map(x=>[x.name,x.role])),JSON.stringify([['Sunidhi Chauhan','Vocalist'],['Divya Kumar','Vocalist']]));
 assert.equal(JSON.stringify(value.credits.genericArtistCreditPreview.map(x=>x.name)),JSON.stringify(['Sunidhi Chauhan','Divya Kumar','Amit Trivedi','Nusrat Fateh Ali Khan','Irshad Kamil']));
 assert.equal(value.credits.rows.some(x=>x.name==='Amit Trivedi'&&x.roleGroup==='Performers'),false);
 assert.equal(value.credits.rows.some(x=>x.name==='Amit Trivedi'&&x.role==='Composer'),true);
 assert.equal(value.credits.rows.some(x=>x.name==='Irshad Kamil'&&x.role==='Lyricist'),true);
 assert.equal(value.credits.rows.some(x=>x.name==='Nusrat Fateh Ali Khan'&&x.role==='Composer'),true);
 const queries=fixture.calls.map(x=>x.options.body&&JSON.parse(x.options.body)).filter(x=>x?.operationName==='queryNpvArtist');
 assert.equal(queries.length,2);
 assert.equal(JSON.stringify(queries.map(x=>x.variables)),JSON.stringify([
  {artistUri:`spotify:artist:${ARTIST_IDS[0]}`,trackUri:`spotify:track:${TRACK_ID}`,contributorsLimit:64,contributorsOffset:0,enableRelatedVideos:false,enableRelatedAudioTracks:false},
  {artistUri:`spotify:artist:${ARTIST_IDS[0]}`,trackUri:`spotify:track:${TRACK_ID}`,contributorsLimit:64,contributorsOffset:10,enableRelatedVideos:false,enableRelatedAudioTracks:false},
 ]));
 assert.equal(queries[0].extensions.persistedQuery.sha256Hash,'e1ae46a21911a3075c1aa29bf09a6c60f9a45b6a2b1132429f10ad06c299b5d7');
 const serialized=JSON.stringify(result);assert.equal(serialized.includes('test-fixture-credential-a1b2c3d4e5'),false);assert.equal(serialized.includes('sp_dc=fixture'),false);
});

test('a Track union without credits or malformed role rows is rejected',async()=>{
 for(const fixture of [fixtureFetch({trackType:'NotFound'}),fixtureFetch({contributors:[{name:'Artist',role:'',roleGroup:{name:'Performers'}}]})]){
  const result=await executeJob(request(),{fetchImpl:fixture.fetchImpl});
  assert.equal(result.ok,false);
 }
 for(const args of [{artistIds:[ARTIST_IDS[0],ARTIST_IDS[0]]},{trackTitle:`${TITLE}\n` }]){
  const result=await executeJob(request(args),{fetchImpl:fixtureFetch().fetchImpl});
  assert.equal(result.ok,false);
 }
});

test('conflicting echoed identity and repeated pagination data fail closed',async()=>{
 for(const fixture of [
  fixtureFetch({trackEcho:{uri:'spotify:track:AAAAAAAAAAAAAAAAAAAAAA'}}),
  fixtureFetch({ignoreOffset:true}),
  fixtureFetch({contributors:[...liveRows,liveRows[0]]}),
  fixtureFetch({includeTotal:true,totalCount:''}),
 ]){
  const result=await executeJob(request(),{fetchImpl:fixture.fetchImpl});
  assert.equal(result.ok,false);
 }
});

test('terminal pagination never implies exhaustive performer-role evidence',async()=>{
 const result=await executeJob(request(),{fetchImpl:fixtureFetch({includeTotal:true}).fetchImpl});
 assert.equal(result.result.spotifyTrackCredits.credits.paginationStatus,'terminal');
 assert.equal(result.result.spotifyTrackCredits.credits.paginationTerminal,true);
 assert.equal(result.result.spotifyTrackCredits.credits.roleEvidenceComplete,false);
 assert.equal(result.result.spotifyTrackCredits.credits.performerCreditsComplete,false);
 assert.equal(result.result.spotifyTrackCredits.credits.classificationReady,false);
 const inconsistent=await executeJob(request(),{fetchImpl:fixtureFetch({includeTotal:true,totalCount:11}).fetchImpl});
 assert.equal(inconsistent.ok,false);
});

test('Pathfinder transport accepts only the pinned read operation needed for credits',async()=>{
 const calls=[];const transport=makeFetcher('sp_dc=fixture',Date.now()+10000,async(url,options)=>{calls.push({url:String(url),options});return Response.json({ok:true});});
 const endpoint='https://api-partner.spotify.com/pathfinder/v2/query';
 const base={variables:{artistUri:`spotify:artist:${ARTIST_IDS[0]}`,trackUri:`spotify:track:${TRACK_ID}`,contributorsLimit:64,contributorsOffset:0,enableRelatedVideos:false,enableRelatedAudioTracks:false},extensions:{persistedQuery:{version:1,sha256Hash:'e1ae46a21911a3075c1aa29bf09a6c60f9a45b6a2b1132429f10ad06c299b5d7'}}};
 await transport(endpoint,{method:'POST',headers:{Authorization:'Bearer fixture-token','Content-Type':'application/json'},body:JSON.stringify({...base,operationName:'queryNpvArtist'})});
 assert.equal(calls.length,1);
 await assert.rejects(transport(endpoint,{method:'POST',headers:{Authorization:'Bearer fixture-token','Content-Type':'application/json'},body:JSON.stringify({...base,operationName:'unknownOperation'})}),/blocked/);
 await assert.rejects(transport(endpoint,{method:'POST',headers:{Authorization:'Bearer fixture-token','Content-Type':'application/json'},body:JSON.stringify({...base,extensions:{persistedQuery:{version:1,sha256Hash:'a'.repeat(64)}},operationName:'queryNpvArtist'})}),/blocked/);
 await assert.rejects(transport(endpoint,{method:'POST',headers:{Authorization:'Bearer fixture-token','Content-Type':'application/json'},body:JSON.stringify({...base,variables:{...base.variables,contributorsOffset:65},operationName:'queryNpvArtist'})}),/blocked/);
 assert.equal(calls.length,1);
});
