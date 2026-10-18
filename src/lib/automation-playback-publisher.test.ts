import { describe, expect, it } from "vitest";
import { createAutomationPlaybackPublisher, queueProjectionRevision } from "./automation-playback-publisher";

const queue = (n: number) => Array.from({ length: n }, (_, i) => ({ id: `t${i}`, artist: "A", title: `Track ${i}` }));

describe("automation playback publisher", () => {
  it("sends full queue initially, then omits it for position-only pulses", () => {
    const publisher = createAutomationPlaybackPublisher("page-a");
    const rows = queue(1000);
    const first = publisher.next({ position: 0 }, "rev-a", rows);
    expect(first.kind).toBe("full");
    expect(first.playback.queue).toHaveLength(1000);
    expect(publisher.acknowledge(first, { protocolVersion: 1, accepted: true })).toBe("accepted");
    const pulse = publisher.next({ position: 1 }, "rev-a", rows);
    expect(pulse.kind).toBe("unchanged");
    expect("queue" in pulse.playback).toBe(false);
    expect(publisher.acknowledge(pulse, { protocolVersion: 1, accepted: true })).toBe("accepted");
  });

  it("sends a full queue after any projection mutation and retries a rejected delta immediately", () => {
    const publisher = createAutomationPlaybackPublisher("page-a");
    const first = publisher.next({ position: 0 }, "rev-a", queue(2));
    publisher.acknowledge(first, { protocolVersion: 1, accepted: true });
    const changed = publisher.next({ position: 1 }, "rev-b", queue(2));
    expect(changed.kind).toBe("full");
    publisher.acknowledge(changed, { protocolVersion: 1, accepted: true });
    const delta = publisher.next({ position: 2 }, "rev-b", queue(2));
    expect(publisher.acknowledge(delta, { protocolVersion: 1, accepted: false, needFull: true })).toBe("retry_full");
    expect(publisher.next({ position: 2 }, "rev-b", queue(2)).kind).toBe("full");
  });

  it("does not let reordered acknowledgements roll the known queue revision backward", () => {
    const publisher = createAutomationPlaybackPublisher("page-a");
    const first = publisher.next({ position: 0 }, "rev-a", queue(1));
    publisher.acknowledge(first, { protocolVersion: 1, accepted: true });
    const old = publisher.next({ position: 1 }, "rev-a", queue(1));
    const newer = publisher.next({ position: 2 }, "rev-b", queue(2));
    publisher.acknowledge(newer, { protocolVersion: 1, accepted: true });
    publisher.acknowledge(old, { accepted: false });
    expect(publisher.next({ position: 3 }, "rev-b", queue(2)).kind).toBe("unchanged");
    const lateMismatch=publisher.next({ position: 4 }, "rev-b", queue(2));
    publisher.acknowledge(lateMismatch,{protocolVersion:1,accepted:true});
    const older=publisher.next({ position: 5 }, "rev-b", queue(2));
    publisher.acknowledge(older,{protocolVersion:1,accepted:true});
    expect(publisher.acknowledge(lateMismatch,{protocolVersion:1,accepted:false,needFull:true})).toBe("rejected");
    expect(publisher.next({ position: 6 }, "rev-b", queue(2)).kind).toBe("unchanged");
  });

  it("revisions occurrence identity changes even when duplicate track metadata is identical", () => {
    const duplicateRows = [{ id: "same", artist: "A", title: "Duplicate" }, { id: "same", artist: "A", title: "Duplicate" }];
    const firstIds = ["entry-a", "entry-b"];
    const swappedIds = ["entry-b", "entry-a"];
    expect(queueProjectionRevision(duplicateRows, firstIds)).not.toBe(queueProjectionRevision(duplicateRows, swappedIds));

    const publisher = createAutomationPlaybackPublisher("page-occurrences");
    const first = publisher.next({ position: 0 }, queueProjectionRevision(duplicateRows, firstIds), duplicateRows, false, firstIds);
    expect(first.playback.queueEntryIds).toEqual(firstIds);
    publisher.acknowledge(first, { protocolVersion: 1, accepted: true });
    const reordered = publisher.next({ position: 1 }, queueProjectionRevision(duplicateRows, swappedIds), duplicateRows, false, swappedIds);
    expect(reordered.kind).toBe("full");
    expect(reordered.playback.queueEntryIds).toEqual(swappedIds);
    publisher.acknowledge(reordered, { protocolVersion: 1, accepted: true });
    const pulse = publisher.next({ position: 2 }, queueProjectionRevision(duplicateRows, swappedIds), duplicateRows, false, swappedIds);
    expect(pulse.kind).toBe("unchanged");
    expect("queueEntryIds" in pulse.playback).toBe(false);
  });

  it("revisions every projected row edit while retaining stable revisions for unchanged rows", () => {
    const rows=queue(2);
    const original=queueProjectionRevision(rows);
    expect(queueProjectionRevision(rows)).toBe(original);
    expect(queueProjectionRevision([{...rows[0]!,title:"Renamed"},rows[1]!])).not.toBe(original);
    expect(queueProjectionRevision([{...rows[0]!,artist:"Different"},rows[1]!])).not.toBe(original);
  });

  it("keeps full-snapshot compatibility with an older native bridge without acknowledgements", () => {
    const publisher=createAutomationPlaybackPublisher("page-old-native");
    const first=publisher.next({position:0},"rev-a",queue(1));
    expect(publisher.acknowledge(first,undefined)).toBe("legacy");
    expect(publisher.next({position:1},"rev-a",queue(1)).kind).toBe("full");
  });

  it("starts a fresh page instance with a full queue", () => {
    const prior = createAutomationPlaybackPublisher("page-old");
    const full = prior.next({ position: 1 }, "rev-a", queue(1));
    prior.acknowledge(full, { protocolVersion: 1, accepted: true });
    expect(createAutomationPlaybackPublisher("page-new").next({ position: 1 }, "rev-a", queue(1)).kind).toBe("full");
  });
});

describe("acknowledged failure history publication", () => {
  const rows = queue(2);
  const history = (revision = "h1", count = 1) => ({revision, failures: Array.from({length:count},(_,index)=>({entryId:`entry-${index}`,reason:"No playable audio match was found"}))});
  it("repeats full history until the receiver acknowledges that exact revision", () => {
    const publisher=createAutomationPlaybackPublisher("page-a");
    const first=publisher.next({position:0},"queue-a",rows,false,undefined,history());
    expect(first.playback.failureHistoryUpdate).toEqual({kind:"full"});
    publisher.acknowledge(first,{protocolVersion:1,accepted:true});
    const noHistoryAck=publisher.next({position:1},"queue-a",rows,false,undefined,history());
    expect(noHistoryAck.kind).toBe("unchanged");
    expect(noHistoryAck.playback.failureHistoryUpdate).toEqual({kind:"full"});
    publisher.acknowledge(noHistoryAck,{protocolVersion:1,accepted:true,failureHistoryRevision:"wrong"});
    const wrongAck=publisher.next({position:2},"queue-a",rows,false,undefined,history());
    expect(wrongAck.playback.diagnostics).toEqual({recentTrackFailures:history().failures});
    publisher.acknowledge(wrongAck,{protocolVersion:1,accepted:true,failureHistoryRevision:"h1"});
    const pulse=publisher.next({position:3,diagnostics:{metric:4}},"queue-a",rows,false,undefined,history());
    expect(pulse.playback.failureHistoryUpdate).toEqual({kind:"unchanged",failureHistoryRevision:"h1"});
    expect(pulse.playback.diagnostics).toEqual({metric:4});
    expect("queue" in pulse.playback).toBe(false);
  });
  it("ignores delayed acknowledgements and prevents older success from undoing a resync", () => {
    const publisher=createAutomationPlaybackPublisher("page-a");
    const old=publisher.next({position:0},"queue-a",rows,false,undefined,history("h1"));
    const repeated=publisher.next({position:1},"queue-a",rows,false,undefined,history("h1"));
    expect(repeated.failureHistoryKind).toBe("full");
    const newer=publisher.next({position:2},"queue-a",rows,false,undefined,history("h2",2));
    publisher.acknowledge(newer,{protocolVersion:1,accepted:true,failureHistoryRevision:"h2"});
    expect(publisher.acknowledge(old,{protocolVersion:1,accepted:true,failureHistoryRevision:"h1"})).toBe("rejected");
    expect(publisher.acknowledge(repeated,{protocolVersion:1,accepted:false,needFull:true})).toBe("rejected");
    const pulse=publisher.next({position:3},"queue-a",rows,false,undefined,history("h2",2));
    expect(pulse.failureHistoryKind).toBe("unchanged");
    const mismatch=publisher.next({position:4},"queue-a",rows,false,undefined,history("h2",2));
    expect(publisher.acknowledge(mismatch,{protocolVersion:1,accepted:false,needFull:true})).toBe("retry_full");
    expect(publisher.acknowledge(pulse,{protocolVersion:1,accepted:true,failureHistoryRevision:"h2"})).toBe("rejected");
    const resync=publisher.next({position:5},"queue-a",rows,false,undefined,history("h2",2));
    expect(resync.kind).toBe("full");
    expect(resync.failureHistoryKind).toBe("full");
    expect(resync.playback.diagnostics).toEqual({recentTrackFailures:history("h2",2).failures});
  });
  it("sends full history for new queues, source resets, page instances, and forced snapshots", () => {
    const publisher=createAutomationPlaybackPublisher("page-a");
    const full=publisher.next({position:0},"queue-a",rows,false,undefined,history());
    publisher.acknowledge(full,{protocolVersion:1,accepted:true,failureHistoryRevision:"h1"});
    expect(publisher.next({position:1},"queue-a",rows,true,undefined,history()).failureHistoryKind).toBe("full");
    const newQueue=publisher.next({position:2},"queue-b",rows,false,undefined,history());
    expect(newQueue.failureHistoryKind).toBe("full");
    publisher.acknowledge(newQueue,{protocolVersion:1,accepted:true,failureHistoryRevision:"h1"});
    const sourceReset=publisher.next({position:3},"queue-b",rows,false,undefined,history("h2",0));
    expect(sourceReset.failureHistoryKind).toBe("full");
    expect(sourceReset.playback.diagnostics).toEqual({recentTrackFailures:[]});
    expect(createAutomationPlaybackPublisher("page-b").next({position:4},"queue-b",rows,false,undefined,history("h2",0)).failureHistoryKind).toBe("full");
  });
  it("keeps acknowledged 64-record position pulses constant and never reads the history array", () => {
    const publisher=createAutomationPlaybackPublisher("page-a");
    const large=history("h64",64);
    large.failures=large.failures.map(item=>({...item,reason:"r".repeat(250),title:"t".repeat(160)}));
    const full=publisher.next({position:0},"queue-a",rows,false,undefined,large);
    expect(JSON.stringify(full.playback).length).toBeGreaterThan(25_000);
    publisher.acknowledge(full,{protocolVersion:1,accepted:true,failureHistoryRevision:"h64"});
    const guarded={revision:"h64",failures:new Proxy(large.failures,{get(){throw new Error("A position pulse read the history array");}})};
    for(let position=1;position<=100;position++) {
      const pulse=publisher.next({position},"queue-a",rows,false,undefined,guarded);
      expect(pulse.failureHistoryKind).toBe("unchanged");
      expect(JSON.stringify(pulse.playback).length).toBeLessThan(400);
      expect(pulse.playback.diagnostics).toEqual({});
    }
  });
});
