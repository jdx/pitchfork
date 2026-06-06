(function () {
  const logSources = new Set();

  window.pitchforkLogEventSource = function (url) {
    const source = new EventSource(url);
    logSources.add(source);
    source.addEventListener("error", function () {
      if (source.readyState === EventSource.CLOSED) {
        logSources.delete(source);
      }
    });
    source.addEventListener("clear", function () {
      document.querySelectorAll('[sse-connect="' + url + '"]').forEach(function (el) {
        el.textContent = '';
      });
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

  window.addEventListener("pagehide", function (evt) {
    if (!evt.persisted) {
      closeLogSources();
    }
  });

  window.addEventListener("pageshow", function (evt) {
    if (evt.persisted) {
      logSources.forEach(function (source) {
        if (source.readyState !== EventSource.OPEN) {
          source.close();
          logSources.delete(source);
        }
      });
    }
  });
})();
