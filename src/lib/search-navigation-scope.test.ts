import {it,expect,vi} from "vitest";
import {searchNavigationScopes} from "./search-navigation-scope";
it("coalesces account-bound providers into one local read with no provider fallback",async()=>{
 const call=vi.fn(async()=>({enabled:true,running:true,spotifyAccountId:"one",youtubeAccountId:"two",accountRevisions:{spotify:3,youtube:4}}));
 expect(await searchNavigationScopes(call,["spotify","youtube","musicbrainz"])).toEqual({spotify:"spotify:revision:3:one",youtube:"youtube:revision:4:two",musicbrainz:"public:musicbrainz"});
 expect(call).toHaveBeenCalledExactlyOnceWith("get_managed_provider_session_status",{});
});
it("unknown state never dispatches provider status, login or background jobs",async()=>{
 const call=vi.fn(async(_command:string,_args:Record<string,unknown>)=>{throw Error("unknown");});
 expect(await searchNavigationScopes(call,["spotify","youtube"])).toEqual({spotify:null,youtube:null});
 expect(call).toHaveBeenCalledTimes(1);expect(call.mock.calls[0][0]).toBe("get_managed_provider_session_status");
});
it("allows revision-bound anonymous public YouTube without requiring login",async()=>{
 const call=vi.fn(async()=>({enabled:false,running:false,youtubeAccountId:null,accountRevisions:{youtube:9}}));
 expect((await searchNavigationScopes(call,["youtube"])).youtube).toBe("youtube:public:revision:9");
 expect((await searchNavigationScopes(vi.fn(async()=>({youtubeReconnectRequired:true,accountRevisions:{youtube:9}})),["youtube"])).youtube).toBeNull();
});
it("does not reuse lingering account identity after shutdown or unknown revision",async()=>{
 for(const status of [{running:false,spotifyAccountId:"old",accountRevisions:{spotify:1}},{spotifyAccountId:"old"}])expect((await searchNavigationScopes(vi.fn(async()=>status),["spotify"])).spotify).toBeNull();
 const call=vi.fn();expect(await searchNavigationScopes(call,["library","soulseek"])).toEqual({library:"public:library",soulseek:null});expect(call).not.toHaveBeenCalled();
});
