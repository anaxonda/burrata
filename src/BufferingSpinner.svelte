<svg class="buffering-spinner" viewBox="0 0 20 20" aria-hidden="true" focusable="false">
  <circle class="buffering-spinner-track" cx="10" cy="10" r="7" />
  <path class="buffering-spinner-arc" d="M10 3a7 7 0 0 1 7 7" />
</svg>

<style>
  .buffering-spinner {
    display: block;
    flex: 0 0 18px;
    width: 18px;
    height: 18px;
    box-sizing: border-box;
    overflow: hidden;
    color: inherit;
  }

  .buffering-spinner-track,
  .buffering-spinner-arc {
    fill: none;
    stroke: currentColor;
    stroke-width: 2.5;
    vector-effect: non-scaling-stroke;
  }

  .buffering-spinner-track {
    opacity: 0.25;
  }

  .buffering-spinner-arc {
    stroke-linecap: round;
    transform-box: view-box;
    transform-origin: center;
    animation: buffering-spinner-rotate 850ms linear infinite;
  }

  @keyframes buffering-spinner-rotate {
    to { transform: rotate(360deg); }
  }

  @media (prefers-reduced-motion: reduce) {
    .buffering-spinner-arc {
      animation: none;
      stroke-dasharray: 1 1.2;
    }
  }
</style>
