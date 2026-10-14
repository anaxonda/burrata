export type StreamingProvider = "monochrome" | "youtube" | "soundcloud";
export function streamingProviderOrder(primary: StreamingProvider, enabled: boolean, configured: string[]): StreamingProvider[] {
  const allowed = ["monochrome", "youtube", "soundcloud"];
  return [primary, ...(enabled ? configured : [])].filter((id, index, all) => allowed.includes(id) && all.indexOf(id) === index) as StreamingProvider[];
}

export function downloadProviderOrder(primary: string, enabled: boolean, configured: string[]): string[] {
  const allowed = ["monochrome", "youtube", "soundcloud", "slskd"];
  return [primary, ...(enabled ? configured : [])].filter((id,index,all)=>allowed.includes(id) && all.indexOf(id)===index);
}
