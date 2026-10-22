// Diagnostic-only harness for the existing Burrata/LibKeyFinder C ABI.
// Input is one mono 22,050 Hz little-endian f32le buffer, as detect_key feeds it.
// This prints raw classifier/profile and production-gate diagnostics; it does not
// change thresholds or write application metadata.
#include <algorithm>
#include <cstdint>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <iterator>
#include <map>
#include <string>
#include <vector>

extern "C" int burrata_keyfinder_analyze(const float*, std::size_t,
                                          std::uint32_t, double*, double*) noexcept;

namespace {
constexpr std::size_t kSampleRate = 22050;
constexpr std::size_t kMinSeconds = 8;
constexpr std::size_t kWindowSeconds = 30;
constexpr double kMinFit = 0.20;
constexpr double kMinMargin = 0.01;
const char* const kNames[] = {"A", "Am", "Bb", "Bbm", "B", "Bm", "C", "Cm",
                              "Db", "Dbm", "D", "Dm", "Eb", "Ebm", "E", "Em",
                              "F", "Fm", "Gb", "Gbm", "G", "Gm", "Ab", "Abm"};
struct Result { int key; double fit; double margin; };
const char* key_name(int key) { return key >= 0 && key < 24 ? kNames[key] : nullptr; }
Result analyze(const float* samples, std::size_t count) {
  Result r{-1, 0.0, 0.0};
  r.key = burrata_keyfinder_analyze(samples, count, kSampleRate, &r.fit, &r.margin);
  return r;
}
void print_key(const char* key) { if (key) std::cout << '"' << key << '"'; else std::cout << "null"; }
}

int main(int argc, char** argv) {
  if (argc != 2) { std::cerr << "usage: keyfinder_diagnostic <mono-22050hz-f32le-file>\n"; return 2; }
  std::ifstream input(argv[1], std::ios::binary | std::ios::ate);
  if (!input) { std::cerr << "cannot open input\n"; return 2; }
  const auto bytes = input.tellg();
  if (bytes < 0 || static_cast<std::uint64_t>(bytes) % sizeof(float) != 0) {
    std::cerr << "input byte length is not a whole number of f32 samples\n"; return 2;
  }
  input.seekg(0);
  std::vector<float> samples(static_cast<std::size_t>(bytes) / sizeof(float));
  input.read(reinterpret_cast<char*>(samples.data()), bytes);
  if (!input) { std::cerr << "cannot read input\n"; return 2; }

  const std::size_t minimum = kSampleRate * kMinSeconds;
  const std::size_t window_length = std::min(samples.size() / 5, kSampleRate * kWindowSeconds);
  Result whole{-1, 0.0, 0.0};
  if (!samples.empty()) whole = analyze(samples.data(), samples.size());
  std::vector<Result> windows;
  std::vector<std::size_t> starts;
  std::map<std::string, unsigned> votes;
  if (samples.size() >= minimum && window_length >= minimum) {
    for (std::size_t part = 0; part < 5; ++part) {
      const std::size_t center = samples.size() * (part * 2 + 1) / 10;
      const std::size_t start = std::min(center > window_length / 2 ? center - window_length / 2 : 0,
                                         samples.size() - window_length);
      const Result r = analyze(samples.data() + start, window_length);
      starts.push_back(start);
      windows.push_back(r);
      if (key_name(r.key) && r.fit >= kMinFit && r.margin >= kMinMargin) ++votes[key_name(r.key)];
    }
  }
  int consensus_key = -1;
  std::string consensus_name;
  unsigned consensus_votes = 0;
  for (const auto& vote : votes) {
    // Matches max_by_key over the BTreeMap's ascending key order: ties take the later key.
    if (vote.second >= consensus_votes && vote.second > 0) {
      consensus_name = vote.first;
      consensus_votes = vote.second;
    }
  }
  for (int k = 0; k < 24; ++k) if (key_name(k) && consensus_name == key_name(k)) consensus_key = k;
  const char* failure = "accepted";
  if (samples.size() < minimum) failure = "minimum_duration";
  else if (!key_name(whole.key)) failure = "whole_classifier_no_key";
  else if (whole.fit < kMinFit) failure = "whole_profile_fit";
  else if (whole.margin < kMinMargin) failure = "whole_profile_margin";
  else if (window_length < minimum) failure = "window_length";
  else if (consensus_votes < 3) failure = "window_consensus_under_3_of_5";
  else if (consensus_key != whole.key) failure = "whole_vs_window_consensus_disagree";

  std::cout << std::fixed << std::setprecision(9)
            << "{\"samples\":" << samples.size()
            << ",\"duration_seconds\":" << static_cast<double>(samples.size()) / kSampleRate
            << ",\"window_samples\":" << window_length
            << ",\"window_seconds\":" << static_cast<double>(window_length) / kSampleRate
            << ",\"thresholds\":{\"profile_fit\":" << kMinFit << ",\"profile_margin\":" << kMinMargin << "}"
            << ",\"whole\":{\"key\":"; print_key(key_name(whole.key));
  std::cout << ",\"profile_fit\":" << whole.fit << ",\"profile_margin\":" << whole.margin
            << ",\"fit_pass\":" << (whole.fit >= kMinFit ? "true" : "false")
            << ",\"margin_pass\":" << (whole.margin >= kMinMargin ? "true" : "false") << "}"
            << ",\"windows\":[";
  for (std::size_t i = 0; i < windows.size(); ++i) {
    if (i) std::cout << ',';
    const auto& r = windows[i];
    std::cout << "{\"index\":" << (i + 1) << ",\"start_seconds\":"
              << static_cast<double>(starts[i]) / kSampleRate << ",\"key\":";
    print_key(key_name(r.key));
    std::cout << ",\"profile_fit\":" << r.fit << ",\"profile_margin\":" << r.margin
              << ",\"qualified\":" << (key_name(r.key) && r.fit >= kMinFit && r.margin >= kMinMargin ? "true" : "false") << '}';
  }
  std::cout << "],\"qualified_consensus\":{\"key\":"; print_key(consensus_name.empty() ? nullptr : consensus_name.c_str());
  std::cout << ",\"votes\":" << consensus_votes << ",\"whole_agrees\":"
            << (consensus_key >= 0 && consensus_key == whole.key ? "true" : "false")
            << "},\"first_production_failure\":\"" << failure << "\"}\n";
}
