// Small C ABI wrapper around the bundled LibKeyFinder API.
// LibKeyFinder is GPL-3.0-or-later; see vendor/libkeyfinder/LICENSE.
#include <cstddef>
#include <cstdint>
#include <limits>

#include "keyfinder.h"

extern "C" int burrata_keyfinder_analyze(const float* samples,
                                          std::size_t sample_count,
                                          std::uint32_t sample_rate,
                                          double* profile_fit,
                                          double* profile_margin) noexcept {
  if (samples == nullptr || sample_count == 0 || sample_rate == 0 ||
      sample_count > std::numeric_limits<unsigned int>::max() ||
      profile_fit == nullptr || profile_margin == nullptr) {
    return -1;
  }

  try {
    KeyFinder::AudioData audio;
    audio.setChannels(1);
    audio.setFrameRate(sample_rate);
    audio.addToSampleCount(static_cast<unsigned int>(sample_count));
    for (unsigned int i = 0; i < static_cast<unsigned int>(sample_count); ++i) {
      audio.setSample(i, static_cast<double>(samples[i]));
    }

    KeyFinder::KeyFinder keyfinder;
    KeyFinder::Workspace workspace;
    keyfinder.progressiveChromagram(audio, workspace);
    keyfinder.finalChromagram(workspace);
    const std::vector<double> chroma = workspace.chromagram->collapseToOneHop();
    const KeyFinder::ToneProfile major(KeyFinder::toneProfileMajor());
    const KeyFinder::ToneProfile minor(KeyFinder::toneProfileMinor());
    double best = -1.0;
    double second = -1.0;
    for (int shift = 0; shift < 12; ++shift) {
      const double major_score = major.cosineSimilarity(chroma, shift);
      const double minor_score = minor.cosineSimilarity(chroma, shift);
      for (double score : {major_score, minor_score}) {
        if (score > best) {
          second = best;
          best = score;
        } else if (score > second) {
          second = score;
        }
      }
    }
    *profile_fit = best;
    *profile_margin = best - second;
    return static_cast<int>(keyfinder.keyOfChromagram(workspace));
  } catch (...) {
    return -1;
  }
}
