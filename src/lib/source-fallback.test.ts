import { expect, it } from "vitest";
import { streamingProviderOrder } from "./source-fallback";
it("uses only the primary when disabled",()=>expect(streamingProviderOrder("monochrome",false,["soundcloud","youtube"])).toEqual(["monochrome"]));
it("honors the configured order and excludes duplicates and unsupported providers",()=>expect(streamingProviderOrder("monochrome",true,["soundcloud","youtube","monochrome","youtube","slskd"])).toEqual(["monochrome","soundcloud","youtube"]));
