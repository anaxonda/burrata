# Provider mark assets

These compact identifiers help users distinguish source names in tight navigation
and metadata rows. Most SVGs are local UI artwork. `bandcamp.png` and `shazam.png`
are 32px favicons downloaded from the providers' public homepages and are used at
small decorative sizes. Provider names, logos, and trademarks remain the property
of their respective owners; these marks do not imply endorsement or an API
partnership.

Before using official artwork or using provider marks in promotion, review the
current official rules:

- Spotify: <https://developer.spotify.com/documentation/design>
- Beatport: <https://support.beatport.com/hc/en-us/articles/4412316336404-Beatport-Logos-and-Images>
- Traxsource: <https://www.traxsource.com/logos-and-images/>

Favicon asset sources (retrieved 2026-10-13):

- Bandcamp: <https://s4.bcbits.com/client-bundle/1/PageLayout_1/favicon-e91a942bb76c26f619d5945a00cf8e60.static/favicon/favicon-32x32.png>
- Shazam: <https://assets.shazam.com/website/images/favicons/favicon-32x32>

`ProviderLogo.svelte` treats the image as decorative and supplies a provider-name
initial when an image cannot load or a provider has no mapped mark.
