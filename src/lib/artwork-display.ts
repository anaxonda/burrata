/** Display-only cache: data URLs never enter Track, playback, or persistence. */
export class ArtworkDisplayCache {
  private values = new Map<string,string>();
  private pending = new Map<string,Promise<string | null>>();
  private attempted = new Set<string>();
  private epoch = 0;
  constructor(private activeUrls:()=>readonly string[]=()=>[]){}
  snapshot():Record<string,string>{return Object.fromEntries(this.values);}
  get(url:string):string | undefined { return this.values.get(url); }
  async recover(url:string,load:(path:string)=>Promise<string>,isCurrent:()=>boolean):Promise<string|null> {
    if (!isCurrent()) return null;
    if (this.values.has(url)) {const value=this.values.get(url)!;this.values.delete(url);this.values.set(url,value);return value;}
    const inflight=this.pending.get(url);if(inflight)return inflight.then(value=>isCurrent()?value:null);
    let path:string;
    try {const parsed=new URL(url);if(parsed.protocol!=="asset:"||parsed.hostname!=="localhost"||parsed.search||parsed.hash)return null;path=decodeURIComponent(parsed.pathname.slice(1));if(!path.startsWith("/"))return null;}catch{return null;}
    if(this.attempted.has(url))return null;
    if(this.pending.size>=3){
      // Await capacity without admitting another native read or retaining image bytes.
      const epoch=this.epoch;
      await Promise.race(this.pending.values());
      if(epoch!==this.epoch||!isCurrent())return null;
      return this.recover(url,load,isCurrent);
    }
    this.attempted.add(url);while(this.attempted.size>12)this.attempted.delete(this.attempted.values().next().value!);
    const epoch=this.epoch;
    const request=(async()=>{
      try {
        const value=await Promise.resolve().then(()=>load(path));
        if(epoch!==this.epoch||value.length>1_398_150||!/^data:image\/(jpeg|png|webp);base64,[A-Za-z0-9+/]+=*$/.test(value))return null;
        this.values.delete(url);this.values.set(url,value);
        // The footer and details can remain visible while unrelated reads finish.
        const active=new Set(this.activeUrls().slice(0,2));
        while(this.values.size>3){const evicted=[...this.values.keys()].find(key=>!active.has(key))!;this.values.delete(evicted);}
        // Successful values may be read again after LRU eviction; only failed reads
        // retain a tombstone. Pending requests and retained values already coalesce.
        this.attempted.delete(url);
        return value;
      }catch{return null;}finally{if(epoch===this.epoch)this.pending.delete(url);}
    })();
    this.pending.set(url,request);return request.then(value=>isCurrent()?value:null);
  }
  clear(){this.epoch++;this.values.clear();this.pending.clear();this.attempted.clear();}
}
