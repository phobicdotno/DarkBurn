/* DarkBurn page: small comforts only. The page is complete without this file. */
(function () {
  'use strict';
  var doc = document;

  function ready(fn) {
    if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', fn);
    else fn();
  }

  /* The section links fold into a menu on narrow screens. */
  function menu() {
    var btn = doc.querySelector('.navbtn');
    var nav = doc.getElementById('nav');
    if (!btn || !nav) return;
    btn.hidden = false;

    function set(open) {
      nav.classList.toggle('is-open', open);
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    }
    btn.addEventListener('click', function () {
      set(btn.getAttribute('aria-expanded') !== 'true');
    });
    nav.addEventListener('click', function (e) {
      if (e.target.closest('a')) set(false);
    });
    doc.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && btn.getAttribute('aria-expanded') === 'true') {
        set(false);
        btn.focus();
      }
    });
    doc.addEventListener('click', function (e) {
      if (!e.target.closest('.top')) set(false);
    });
  }

  /* Copy buttons beside the install commands. */
  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var back = doc.activeElement;
      var ta = doc.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      doc.body.appendChild(ta);
      ta.select();
      var ok = false;
      try { ok = doc.execCommand('copy'); } catch (err) { ok = false; }
      doc.body.removeChild(ta);
      if (back && back.focus) back.focus();
      if (ok) resolve(); else reject(new Error('copy failed'));
    });
  }

  function copyButtons() {
    var boxes = doc.querySelectorAll('.cmd');
    Array.prototype.forEach.call(boxes, function (box) {
      var code = box.querySelector('pre code');
      var head = box.querySelector('.cmd__head');
      var title = head && head.querySelector('h3');
      if (!code || !head) return;

      var btn = doc.createElement('button');
      btn.type = 'button';
      btn.className = 'copy';
      btn.textContent = 'Copy';
      if (title) btn.setAttribute('aria-label', 'Copy the command for ' + title.textContent);

      var said = doc.createElement('span');
      said.setAttribute('role', 'status');
      said.style.cssText = 'position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap';

      var timer;
      function show(label, done) {
        btn.textContent = label;
        btn.classList.toggle('is-done', done);
        said.textContent = done ? 'Copied' : '';
        clearTimeout(timer);
        timer = setTimeout(function () {
          btn.textContent = 'Copy';
          btn.classList.remove('is-done');
          said.textContent = '';
        }, 2000);
      }
      btn.addEventListener('click', function () {
        copyText(code.textContent.trim()).then(
          function () { show('Copied', true); },
          function () {
            var range = doc.createRange();
            range.selectNodeContents(code);
            var sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            show('Selected', false);
          }
        );
      });
      head.appendChild(btn);
      head.appendChild(said);
    });
  }

  /* Mark the section in view in the top bar. */
  function spy() {
    if (!('IntersectionObserver' in window)) return;
    var links = doc.querySelectorAll('#nav a[href^="#"]');
    var seen = {};
    function mark() {
      var found = false;
      Array.prototype.forEach.call(links, function (a) {
        var on = !found && seen[a.getAttribute('href').slice(1)];
        if (on) { a.setAttribute('aria-current', 'true'); found = true; }
        else a.removeAttribute('aria-current');
      });
    }
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) { seen[e.target.id] = e.isIntersecting; });
      mark();
    }, { rootMargin: '-25% 0px -70% 0px' });
    Array.prototype.forEach.call(links, function (a) {
      var el = doc.getElementById(a.getAttribute('href').slice(1));
      if (el) io.observe(el);
    });
  }

  /* On a wide screen a picture opens large on the page, with the others one key away.
     On a narrow one, and without this file, the link opens the picture itself. */
  function viewer() {
    var shots = Array.prototype.slice.call(doc.querySelectorAll('a.shot'));
    var probe = doc.createElement('dialog');
    if (!shots.length || typeof probe.showModal !== 'function' || !window.matchMedia) return;
    var wide = window.matchMedia('(min-width: 900px)');
    var box, img, cap, count, at = 0;

    function el(tag, cls, text) {
      var e = doc.createElement(tag);
      if (cls) e.className = cls;
      if (text) e.textContent = text;
      return e;
    }
    function button(text, label, fn) {
      var b = el('button', 'zoom__btn', text);
      b.type = 'button';
      b.setAttribute('aria-label', label);
      b.addEventListener('click', fn);
      return b;
    }
    function captionOf(a) {
      var fig = a.closest('figure');
      var fc = fig && fig.querySelector('figcaption');
      if (!fc) return '';
      var head = fc.querySelector('h3, h4');
      var text = fc.querySelector('p:last-child');
      if (!head) return fc.textContent.trim();
      var name = head.textContent.trim();
      var words = text ? text.textContent.trim() : '';
      return words.indexOf(name) === 0 ? words : name + '. ' + words;
    }
    function show(i) {
      at = (i + shots.length) % shots.length;
      var a = shots[at];
      var small = a.querySelector('img');
      img.src = a.getAttribute('href');
      img.alt = small ? small.alt : '';
      cap.textContent = captionOf(a);
      count.textContent = (at + 1) + ' / ' + shots.length;
    }
    function build() {
      box = el('dialog', 'zoom');
      box.setAttribute('aria-label', 'Picture at full size');
      var bar = el('div', 'zoom__bar');
      count = el('p', 'zoom__count');
      count.setAttribute('aria-live', 'polite');
      img = el('img', 'zoom__img');
      img.width = 2880;
      img.height = 1920;
      cap = el('p', 'zoom__cap');
      bar.appendChild(count);
      bar.appendChild(button('\u2190', 'Previous picture', function () { show(at - 1); }));
      bar.appendChild(button('\u2192', 'Next picture', function () { show(at + 1); }));
      var close = button('Close', 'Close the picture', function () { box.close(); });
      close.autofocus = true;
      bar.appendChild(close);
      box.appendChild(bar);
      box.appendChild(img);
      box.appendChild(cap);
      box.addEventListener('click', function (e) {
        if (e.target === box) box.close();
      });
      box.addEventListener('keydown', function (e) {
        if (e.key === 'ArrowLeft') { show(at - 1); e.preventDefault(); }
        if (e.key === 'ArrowRight') { show(at + 1); e.preventDefault(); }
      });
      box.addEventListener('close', function () {
        img.removeAttribute('src');
      });
      doc.body.appendChild(box);
    }
    shots.forEach(function (a, i) {
      a.addEventListener('click', function (e) {
        if (!wide.matches || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
        e.preventDefault();
        if (!box) build();
        show(i);
        box.showModal();
      });
    });
  }

  ready(function () {
    menu();
    copyButtons();
    spy();
    viewer();
  });
})();
