/* The map's one piece of script, inlined by `map.rs` (`WEB.md` §5: one
   binary, no build step, and the CSP admits `script-src 'unsafe-inline'`
   and no external script at all).

   What it is for, and the only thing it is for: the promise `d391` signs is
   that you read why a node exists *without letting go of the tree you found
   it in*. Following a link is letting go. So the click that would navigate
   is intercepted, the detail is filled in from data the page already
   carries, and the drawing stays exactly where it was.

   Everything below degrades to the product as it was: with script off the
   rails, the stations and the rows are all still drawn by the server, and
   every alias is a real link to its own lineage. Nothing here reaches for
   anything -- there is no fetch in this file, and there is no network in
   the CSP for one to use. */

(function () {
  var box = document.getElementById("map-data");
  var panel = document.getElementById("detail");
  if (!box || !panel) return;

  var data = JSON.parse(box.textContent);
  var D = data.stops;
  var project = data.project;

  var map = document.querySelector(".map");
  var stations = [].slice.call(document.querySelectorAll("svg .station"));
  var gutters = [].slice.call(document.querySelectorAll("svg.rails"));

  /* The row of each stop that has one, by stop. Not an array in document
     order: the payload carries every node and only some of them are drawn,
     so a position in the list is not a position in the payload. */
  var rows = {};
  [].slice.call(document.querySelectorAll("li.stop")).forEach(function (e) {
    rows[+e.dataset.stop] = e;
  });
  var drawn = Object.keys(rows).map(Number);

  function eachRow(fn) {
    drawn.forEach(function (k) {
      fn(rows[k], k);
    });
  }
  /* What the panel says when nothing is chosen. Rendered by the server so
     it is there with no script at all, and kept here so closing the panel
     puts it back rather than leaving a hole. */
  var resting = panel.innerHTML;
  var at = null;

  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  /* The route to a stop is its ancestor chain, root first. It is the same
     walk `vivac why` prints, which is the point: this page answers the
     question that command answers, without the cost that command charges. */
  function routeTo(i) {
    var out = [];
    while (i !== null && i !== undefined) {
      out.unshift(i);
      i = D[i].p;
    }
    return out;
  }

  /* The route, drawn over the rails in each gutter. Each gutter carries its
     own lane width in `data-` attributes rather than this file knowing the
     two numbers: the server computed the drawing and the server is the one
     place they are decided. */
  function drawRoute(route) {
    gutters.forEach(function (svg) {
      var g = svg.querySelector(".route");
      g.innerHTML = "";
      if (!route.length) return;

      var step = +svg.dataset.step,
        origin = +svg.dataset.origin,
        row = +svg.dataset.row;
      var x = function (i) {
        return origin + D[i].d * step;
      };
      /* The row a stop is drawn on, never its place in the payload: the
         payload holds the whole tree and the drawing holds what is left
         after a fold. */
      var y = function (i) {
        return D[i].r * row + row / 2;
      };

      var d = "";
      route.forEach(function (n, k) {
        if (k === 0) {
          d += "M" + x(n) + " " + y(n);
          return;
        }
        /* Down the parent's lane, then the same quarter turn into the
           child's that the elbow underneath already draws. */
        var px = x(route[k - 1]);
        d +=
          " V" + (y(n) - 7) + " Q" + px + " " + y(n) + " " + (px + 7) +
          " " + y(n) + " H" + x(n);
      });
      g.insertAdjacentHTML("beforeend", '<path d="' + d + '"/>');
      route.forEach(function (n) {
        g.insertAdjacentHTML(
          "beforeend",
          '<circle cx="' + x(n) + '" cy="' + y(n) + '" r="4.2"/>'
        );
      });
    });
  }

  function section(head, body) {
    return body ? "<h3>" + head + "</h3><p class=\"prose\">" + esc(body) + "</p>" : "";
  }

  function links(head, list) {
    if (!list.length) return "";
    return (
      "<h3>" + head + " · " + list.length + "</h3><ul class=\"plain\">" +
      list
        .map(function (k) {
          return (
            '<li><a class="go" href="#" data-go="' + k + '">' + esc(D[k].a) +
            "</a>" + esc(D[k].t) + "</li>"
          );
        })
        .join("") +
      "</ul>"
    );
  }

  /* What holds a node open. `bn` is how many there are and `bl` only the
     ones still on the page: a blocker is always a descendant, so folding a
     node hides its own debts, and a panel that listed the survivors alone
     would say a node is waiting on nothing while it waits on three. */
  function waiting(n) {
    if (!n.bn) return "";
    var out = links("Does not close until these close", n.bl);
    if (!out) out = "<h3>Does not close until these close · " + n.bn + "</h3>";
    var away = n.bn - n.bl.length;
    if (away > 0) {
      out +=
        '<p class="prose">' + away + " of them " + (away === 1 ? "is" : "are") +
        " folded away.</p>";
    }
    return out;
  }

  /* Every note, oldest first, each with the date it was written. Until
     9-Sep-2026 the product kept only the last one a node was given and 84
     of 225 were unreadable anywhere (`f389`); a panel that showed one again
     would be that defect coming back through a different door. */
  function notes(n) {
    if (!n.nt.length) return "";
    var many = n.nt.length > 1;
    return (
      "<h3>Notes · " + n.nt.length + "</h3>" +
      n.nt
        .map(function (x, k) {
          var last = k === n.nt.length - 1;
          return (
            "<details" + (last || !many ? " open" : "") + "><summary>" +
            (many ? "note " + (k + 1) + " · " : "") +
            '<span class="when">' + esc(x.at.slice(0, 10)) + "</span>" +
            "</summary><p class=\"prose\">" + esc(x.n) + "</p></details>"
          );
        })
        .join("")
    );
  }

  /* What is beside it and what is under it.
     A tree drawn in one column puts a node's siblings as far apart as the
     work between them is deep -- on the real tree, hundreds of rows -- so
     "the next one along" is the hardest thing on this page to reach by
     scrolling. These are the same two lists the drawing already holds; they
     just cost nothing to name. */
  function around(i) {
    var n = D[i];
    var beside = [];
    var under = [];
    D.forEach(function (o, k) {
      if (k !== i && o.p === n.p && n.p !== null) beside.push(k);
      if (o.p === i) under.push(k);
    });
    return links("Beside it", beside) + links("Under it", under);
  }

  function card(n) {
    var pairs = [
      ["line", n.ln || "a short branch"],
      ["under", n.p === null ? "a root" : D[n.p].a],
      ["children", n.c],
      ["below", n.tb + " nodes, " + n.ob + " open"],
      ["opened", n.op || "--"],
    ];
    if (n.cl) pairs.push(["closed", n.cl]);
    if (n.rf.length) pairs.push(["refs", n.rf.join("<br>")]);
    if (n.gv.length) pairs.push(["governs", n.gv.join(", ")]);
    return (
      "<h3>Card</h3><dl>" +
      pairs
        .map(function (p) {
          return "<dt>" + p[0] + "</dt><dd>" + p[1] + "</dd>";
        })
        .join("") +
      "</dl>"
    );
  }

  function show(i, quiet) {
    var n = D[i];
    /* A node with no row is folded away: there is nothing to draw a route
       to and nothing to scroll to. The search offers a link that unfolds it
       instead, which is the only honest answer. */
    if (!n || n.r === null) return;
    var route = routeTo(i);
    var onRoute = {};
    route.forEach(function (k) {
      onRoute[k] = true;
    });

    eachRow(function (e, k) {
      e.classList.toggle("on", k === i);
      e.classList.toggle("onroute", !!onRoute[k]);
    });
    stations.forEach(function (e) {
      e.classList.toggle("on", !!onRoute[+e.dataset.stop]);
    });
    map.classList.add("routing");
    drawRoute(route);

    panel.innerHTML =
      '<div class="sheet"><span class="grab"></span>' +
      '<span class="code">' + esc(n.a) + "</span>" +
      '<button class="close" type="button">Close</button></div>' +
      '<p class="code">' + esc(n.a) + " · " + esc(n.k) + " · " + esc(n.s) +
      (n.b ? " · blocks its parent" : "") +
      (n.fc ? " · FALSE CLOSE" : "") + "</p>" +
      "<h2>" + esc(n.t) + "</h2>" +
      section("Why it was born", n.w) +
      section("Outcome", n.o) +
      waiting(n) +
      notes(n) +
      around(i) +
      "<h3>The route here · " + route.length + " stops</h3>" +
      '<ol class="plain route-list">' +
      route
        .map(function (k, j) {
          return (
            '<li class="' + (j === route.length - 1 ? "last" : "") + '">' +
            '<a class="go" href="#" data-go="' + k + '">' + esc(D[k].a) +
            "</a>" + esc(D[k].t) + "</li>"
          );
        })
        .join("") +
      "</ol>" +
      card(n) +
      '<p class="onward"><a href="/p/' + encodeURIComponent(project) +
      "/why/" + encodeURIComponent(n.a) + '">The full lineage, on its own page</a></p>';

    /* On a phone the panel is a drawer: it only comes up when a person
       asked for a node, never when the keyboard is walking the list. */
    if (!quiet) panel.classList.add("open");
    at = i;

    /* The selection goes into the address bar, so folding something -- which
       reloads the page -- comes back to the node you were reading, and so a
       view of the tree is a link you can send. `replaceState` rather than a
       hash assignment: it leaves no history entry per keystroke and fires no
       `hashchange` for the listener below to answer. */
    try {
      history.replaceState(null, "", "#" + encodeURIComponent(n.a));
    } catch (e) {
      /* A page opened from a file has no history to replace. */
    }
  }

  function jump(i, straight) {
    var e = rows[i];
    if (!e) return;
    /* Smooth when a person asked to move, instant when the page is
       arriving. A fold reloads the page, and animating twelve thousand
       pixels of scroll after a reload reads as landing at the top and then
       sliding -- which is exactly how the owner described losing their
       place. */
    e.scrollIntoView({ block: "center", behavior: straight ? "auto" : "smooth" });
  }

  function rest() {
    panel.classList.remove("open");
    map.classList.remove("routing");
    panel.innerHTML = resting;
    eachRow(function (e) {
      e.classList.remove("on", "onroute");
    });
    stations.forEach(function (e) {
      e.classList.remove("on");
    });
    drawRoute([]);
    at = null;
  }

  /* One listener on the panel rather than one per link, because the panel's
     contents are rewritten on every selection. */
  panel.addEventListener("click", function (e) {
    var go = e.target.closest("[data-go]");
    if (go) {
      e.preventDefault();
      var to = +go.dataset.go;
      show(to);
      jump(to);
      return;
    }
    if (e.target.closest(".close")) rest();
  });

  eachRow(function (e) {
    e.addEventListener("click", function (ev) {
      var link = ev.target.closest("a");
      /* The fold control is a link that has to *navigate*: folding
         recomputes the map, and the server is the only place that drawing
         is implemented. Swallowing this click would leave a control that
         looks like a link and does nothing. */
      if (link && link.classList.contains("fold")) return;
      /* The alias is a real link and stays one: with no script it is how
         you read a node. Here it opens the panel instead, which is the
         whole difference this page is for. */
      if (link) ev.preventDefault();
      show(+e.dataset.stop);
    });
  });

  stations.forEach(function (e) {
    e.addEventListener("click", function () {
      var i = +e.dataset.stop;
      show(i);
      jump(i);
    });
  });

  var here = document.getElementById("here");
  if (here) {
    here.addEventListener("click", function () {
      var i = +here.dataset.stop;
      show(i);
      jump(i);
    });
  }

  /* The legend highlights a line; it does not filter one. Nothing leaves
     the page, because the rails are drawn by row index and a row that goes
     missing takes its rail's meaning with it (`f394`). */
  document.querySelectorAll(".legend .line").forEach(function (b) {
    b.addEventListener("click", function () {
      var on = b.classList.toggle("on");
      document.querySelectorAll(".legend .line").forEach(function (o) {
        if (o !== b) o.classList.remove("on");
      });
      eachRow(function (e, k) {
        e.classList.toggle("faded", on && D[k].ln !== b.dataset.line);
      });
    });
  });

  /* The way to a node that is folded away: drop whatever ancestor is
     folded over it, and raise the depth if that is what is in the way.
     Built here rather than on the server because only the search knows
     which nodes it wants to reach. */
  function reach(list) {
    var open = {};
    var need = 0;
    list.forEach(function (i) {
      if (D[i].r !== null) return;
      need = Math.max(need, D[i].d + 1);
      var k = D[i].p;
      while (k !== null && k !== undefined) {
        if (folded[D[k].a]) open[D[k].a] = true;
        k = D[k].p;
      }
    });
    var keep = (data.fold || []).filter(function (a) {
      return !open[a];
    });
    var depth = data.depth;
    if (depth && need > depth) depth = need;
    var parts = [];
    if (depth) parts.push("depth=" + depth);
    if (keep.length) parts.push("fold=" + keep.join(","));
    return "?" + parts.join("&");
  }

  var folded = {};
  (data.fold || []).forEach(function (a) {
    folded[a] = true;
  });

  var find = document.getElementById("find");
  var hits = document.getElementById("hits");
  var found = [];
  var here = [];
  var cursor = -1;
  if (find) {
    find.addEventListener("input", function () {
      var v = find.value.trim().toLowerCase();
      eachRow(function (e) {
        e.classList.remove("hit");
      });
      found = [];
      cursor = -1;
      if (v.length > 1) {
        /* The whole tree, and not what is on screen. Folding is a way of
           looking; it is not a claim that the rest stopped existing, and a
           search that quietly skipped two hundred nodes said otherwise. */
        D.forEach(function (n, i) {
          if ((n.a + " " + n.t + " " + n.w).toLowerCase().indexOf(v) >= 0) {
            found.push(i);
            if (rows[i]) rows[i].classList.add("hit");
          }
        });
      }
      here = found.filter(function (i) {
        return D[i].r !== null;
      });
      var away = found.length - here.length;
      if (v.length < 2) {
        hits.textContent = "";
      } else if (!found.length) {
        hits.textContent = "nothing found";
      } else {
        hits.textContent =
          found.length + " found" + (here.length ? " · Enter walks them" : "");
        if (away) {
          hits.insertAdjacentHTML(
            "beforeend",
            ' · <a href="' + reach(found) + '">' + away + " folded away</a>"
          );
        }
      }
    });
    find.addEventListener("keydown", function (e) {
      if (e.key !== "Enter" || !here.length) return;
      e.preventDefault();
      cursor = (cursor + (e.shiftKey ? -1 : 1) + here.length) % here.length;
      var i = here[cursor];
      show(i, true);
      jump(i);
      hits.textContent = cursor + 1 + " of " + here.length + " on the page";
    });
  }

  document.addEventListener("keydown", function (e) {
    if (e.target === find) return;
    if (e.key === "Escape") return rest();
    if (e.key === "/") {
      e.preventDefault();
      if (find) find.focus();
      return;
    }
    /* Up and down walk the rows on the page, not the payload: the payload
       is the whole tree and half of it may be folded away. */
    if (e.key === "ArrowDown" || e.key === "j" || e.key === "ArrowUp" || e.key === "k") {
      e.preventDefault();
      var back = e.key === "ArrowUp" || e.key === "k";
      var seat = at === null ? -1 : drawn.indexOf(at);
      var to =
        seat < 0
          ? drawn[0]
          : drawn[Math.min(Math.max(seat + (back ? -1 : 1), 0), drawn.length - 1)];
      show(to, true);
      jump(to);
    }
  });

  /* A node named in the address bar puts you *at* that row, and does not
     open it.

     The two are different things and the first version ran them together:
     the fold controls carry the node they fold as their fragment, so that
     folding brings you back to where you were standing -- and every fold
     click therefore also selected that node, dimmed the rest of the map and
     threw the drawer open on a phone. A control that folds should fold.

     So this scrolls, and selecting stays what it always was: a click. The
     row you land on is marked by `:target`, which is the browser's own
     answer to "which one did the link mean" and needs no script at all. */
  function fromHash() {
    var alias = decodeURIComponent(location.hash.replace(/^#/, ""));
    if (!alias) return;
    for (var i = 0; i < D.length; i++) {
      if (D[i].a === alias) {
        jump(i, true);
        return;
      }
    }
  }
  window.addEventListener("hashchange", fromHash);
  fromHash();
})();
