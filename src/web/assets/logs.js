(function () {
  const logSources = new Set();

  window.pitchforkLogEventSource = function (url) {
    const source = new EventSource(url, { withCredentials: true });
    logSources.add(source);
    source.addEventListener("error", function () {
      if (source.readyState === EventSource.CLOSED) {
        logSources.delete(source);
      }
    });
    return source;
  };

  if (window.htmx) {
    htmx.createEventSource = window.pitchforkLogEventSource;
  }

  function closeLogSources() {
    logSources.forEach(function (source) {
      source.close();
    });
    logSources.clear();
  }

  window.addEventListener("pagehide", closeLogSources);
  window.addEventListener("beforeunload", closeLogSources);
  document.addEventListener(
    "click",
    function (evt) {
      const link = evt.target.closest && evt.target.closest("a[href]");
      if (link && link.target !== "_blank") {
        closeLogSources();
      }
    },
    true,
  );
})();
