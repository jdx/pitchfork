<template>
  <section
    aria-labelledby="endev-sponsors-title"
    class="EndevSponsors"
  >
    <div class="EndevSponsorsInner">
      <p id="endev-sponsors-title" class="EndevSponsorsTitle">
        sponsors
      </p>
      <a class="EndevSponsorsCta" href="https://github.com/sponsors/jdx">
        Sponsor on GitHub
      </a>
    </div>
  </section>
</template>

<style scoped>
.EndevSponsors {
  padding: 22px 24px;
}

.EndevSponsorsInner {
  align-items: center;
  display: flex;
  flex-wrap: wrap;
  gap: 12px 18px;
  justify-content: center;
  margin: 0 auto;
  max-width: 960px;
}

.EndevSponsorsTitle {
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 600;
  margin: 0;
  text-transform: uppercase;
}

.EndevSponsorsLogos {
  align-items: center;
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  justify-content: center;
}

.EndevSponsorsError {
  color: var(--vp-c-text-3);
  font-size: 13px;
  font-weight: 500;
  margin: 0;
}

.EndevSponsorsLogo {
  align-items: center;
  border: 1px solid var(--vp-c-divider);
  border-radius: 8px;
  display: inline-flex;
  height: 40px;
  justify-content: center;
  padding: 8px 12px;
  transition: border-color 0.2s ease, background-color 0.2s ease;
}

.EndevSponsorsLogo:hover {
  background: var(--vp-c-bg-soft);
  border-color: var(--vp-c-brand-1);
}

.EndevSponsorsLogo img {
  display: block;
  max-height: 22px;
  max-width: 120px;
}

.EndevSponsorsCta {
  color: var(--vp-c-text-2);
  font-size: 13px;
  font-weight: 500;
  text-decoration: none;
  transition: color 0.2s ease;
}

.EndevSponsorsCta:hover {
  color: var(--vp-c-brand-1);
}
</style>
