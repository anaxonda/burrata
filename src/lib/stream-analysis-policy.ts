export type StreamAnalysisRestorePolicy = {
  reviewRequired: boolean;
  categories: Array<{ kind: string; paused: boolean }>;
};

export function isStreamMediaAnalysisPaused(policy: StreamAnalysisRestorePolicy): boolean {
  const category = policy.categories.find((item) => item.kind === "background_stream_media_analysis");
  return category ? category.paused : policy.reviewRequired;
}
