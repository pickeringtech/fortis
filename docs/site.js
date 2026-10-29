// Search over both documentation modes and the specification (conformance/tools/site.py writes search.js, which
// defines window.FORTIS_SEARCH: one entry per section, {u: url from the site root, p: page, h: heading, m: mode, x: text}).
(function () {
  "use strict";
  var input = document.getElementById("search");
  var box = document.getElementById("results");
  var root = document.body.getAttribute("data-root") || ".";
  var index = window.FORTIS_SEARCH || [];
  if (!input || !box) return;
  var MODE = { human: "guide", agent: "agent", spec: "spec", home: "home" };

  function escape(s) {
    return s.replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; });
  }

  function terms(q) {
    return q.toLowerCase().split(/\s+/).filter(function (t) { return t.length > 0; });
  }

  function score(entry, ts) {
    var h = (entry.h + " " + entry.p).toLowerCase();
    var x = entry.x.toLowerCase();
    var s = 0;
    for (var i = 0; i < ts.length; i++) {
      var t = ts[i];
      var inH = h.indexOf(t) >= 0;
      var inX = x.indexOf(t) >= 0;
      if (!inH && !inX) return 0;
      s += (inH ? 10 : 0) + (inX ? 1 : 0);
      if (/^(lang|prog|plat)-|^e-[a-z]+-\d/.test(t) && inX) s += 5;
    }
    return s;
  }

  function snippet(text, ts) {
    var lower = text.toLowerCase();
    var at = lower.indexOf(ts[0]);
    var start = Math.max(0, at - 60);
    var piece = (start > 0 ? "…" : "") + text.slice(start, start + 200) + (text.length > start + 200 ? "…" : "");
    var html = escape(piece);
    ts.forEach(function (t) {
      var re = new RegExp("(" + t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "ig");
      html = html.replace(re, "<mark>$1</mark>");
    });
    return html;
  }

  var active = -1;

  function show(q) {
    var ts = terms(q);
    if (!ts.length) { box.hidden = true; return; }
    var hits = [];
    for (var i = 0; i < index.length; i++) {
      var s = score(index[i], ts);
      if (s > 0) hits.push([s, index[i]]);
    }
    hits.sort(function (a, b) { return b[0] - a[0]; });
    // a long section is indexed in pieces: show it once
    var seen = {};
    hits = hits.filter(function (h) { if (seen[h[1].u]) return false; seen[h[1].u] = true; return true; });
    hits = hits.slice(0, 40);
    if (!hits.length) {
      box.innerHTML = '<div class="where">No section mentions ' + escape(q) + ".</div>";
    } else {
      box.innerHTML = hits.map(function (h) {
        var e = h[1];
        return '<a class="hit" href="' + root + "/" + e.u + '"><span class="m ' + e.m + '">' + MODE[e.m] + "</span>" +
          escape(e.h) + '<div class="where">' + escape(e.p) + '</div><div class="snippet">' + snippet(e.x, ts) + "</div></a>";
      }).join("");
    }
    active = -1;
    box.hidden = false;
  }

  input.addEventListener("input", function () { show(input.value); });
  input.addEventListener("keydown", function (ev) {
    var links = box.querySelectorAll("a.hit");
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      if (!links.length) return;
      ev.preventDefault();
      active = (active + (ev.key === "ArrowDown" ? 1 : links.length - 1)) % links.length;
      links.forEach(function (l, i) { l.classList.toggle("active", i === active); });
      links[active].scrollIntoView({ block: "nearest" });
    } else if (ev.key === "Enter" && links.length) {
      window.location.href = links[Math.max(active, 0)].getAttribute("href");
    } else if (ev.key === "Escape") {
      box.hidden = true;
    }
  });
  document.addEventListener("click", function (ev) {
    if (ev.target !== input && !box.contains(ev.target)) box.hidden = true;
  });
  document.addEventListener("keydown", function (ev) {
    if (ev.key === "/" && document.activeElement !== input) { ev.preventDefault(); input.focus(); }
  });
})();
