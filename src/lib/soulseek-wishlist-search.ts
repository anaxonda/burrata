export type WishlistCall = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
function wait(signal: AbortSignal): Promise<void> {
  return new Promise((resolve,reject) => {
    const abort = () => {clearTimeout(timer);reject(new Error("Search cancelled"));};
    const timer = setTimeout(() => {signal.removeEventListener("abort",abort);resolve();},1000);
    signal.addEventListener("abort",abort,{once:true});
    if(signal.aborted) abort();
  });
}
/** Uses ordinary network search; never selects or downloads a returned candidate. */
export async function searchWishlist(call: WishlistCall, query: string, parent: AbortSignal): Promise<number> {
  const controller=new AbortController();
  const abort=()=>controller.abort();parent.addEventListener("abort",abort,{once:true});
  const timeout=setTimeout(abort,45_000);
  let id: string | null=null;
  try {
    if(parent.aborted) controller.abort();
    controller.signal.throwIfAborted();
    const started=await call<{id:string}>("start_slskd_search",{query});id=started.id;
    if(typeof id!=="string" || !id) throw new Error("Search did not return an identifier");
    for(let polls=0;polls<30;polls++) {
      controller.signal.throwIfAborted();
      const page=await call<{complete:boolean;candidates:{id:string}[];warnings?:string[]}>("get_slskd_search",{searchId:id});
      controller.signal.throwIfAborted();
      if(page.complete) {
        if(!Array.isArray(page.candidates)) throw new Error("Invalid search response");
        if(page.warnings?.length && !page.candidates.length) throw new Error("Search incomplete");
        return new Set(page.candidates.map(c=>c.id)).size;
      }
      await wait(controller.signal);
    }
    throw new Error("Search observation deadline exceeded");
  } finally {
    clearTimeout(timeout);parent.removeEventListener("abort",abort);
    if(id) await call("cancel_slskd_search",{searchId:id}).catch(()=>{});
  }
}
