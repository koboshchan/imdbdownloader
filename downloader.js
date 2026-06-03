#!/usr/bin/env node

'use strict';

const { execSync, spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const os = require('os');
const readline = require('readline');
const zlib = require('zlib');
const axios = require('axios');
const { program } = require('commander');

// ── Global config ─────────────────────────────────────────────────────────────

const config = {
  noSubs: false,
  embedSubs: false,
  subLang: 'English',
};

const SUB_BASE = 'https://feliratok.eu/index.php';

// ── HTTP helpers ──────────────────────────────────────────────────────────────

async function fetchURL(url) {
  try {
    const res = await axios.get(url, {
      headers: {
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0',
        'Referer': 'https://brightpathsignals.com/',
      },
      maxRedirects: 10,
      timeout: 30000,
      transformResponse: [data => data],
    });
    return res.data || '';
  } catch (e) {
    console.error('Fetch error:', e.message);
    return '';
  }
}

async function fetchSubAPI(queryParams) {
  const url = `${SUB_BASE}?${queryParams}`;
  try {
    const res = await axios.get(url, {
      headers: { 'User-Agent': 'xbmc subtitle plugin' },
      maxRedirects: 10,
      timeout: 30000,
      transformResponse: [data => data],
    });
    return res.data || '';
  } catch {
    return '';
  }
}

async function fetchOpenSubtitles(url) {
  try {
    const res = await axios.get(url, {
      headers: {
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:153.0) Gecko/20100101 Firefox/153.0',
        'X-User-Agent': 'trailers.to-UA',
        'Accept': '*/*',
        'Referer': 'https://brightpathsignals.com/',
      },
      maxRedirects: 10,
      timeout: 30000,
      transformResponse: [data => data],
    });
    return res.data || '';
  } catch {
    return '';
  }
}

async function downloadBinaryFile(url, filepath, extraHeaders = {}) {
  try {
    const res = await axios.get(url, {
      headers: {
        'User-Agent': 'xbmc subtitle plugin',
        'Accept': '*/*',
        ...extraHeaders,
      },
      maxRedirects: 10,
      responseType: 'arraybuffer',
      timeout: 60000,
    });
    fs.writeFileSync(filepath, Buffer.from(res.data));
    return true;
  } catch {
    return false;
  }
}

async function downloadWyzie(url, filepath) {
  return downloadBinaryFile(url, filepath, {
    'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:153.0) Gecko/20100101 Firefox/153.0',
    'Referer': 'https://brightpathsignals.com/',
  });
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function stripToJSON(s) {
  const p = s.search(/[{[]/);
  return p === -1 ? s : s.slice(p);
}

function sanitizeFilename(name) {
  return name.replace(/ /g, '_').replace(/[^a-zA-Z0-9_\-]/g, '');
}

function stripYear(title) {
  let t = title.replace(/ \(\d{4}\)$/, '').replace(/ \d{4}$/, '');
  t = t.replace(/[^a-zA-Z0-9]+$/, '');
  return t;
}

// ── Language maps ─────────────────────────────────────────────────────────────

const LANG_HUN = {
  English: 'angol',    Hungarian: 'magyar',   Spanish: 'spanyol',
  French: 'francia',   German: 'n\u00e9met',  Italian: 'olasz',
  Japanese: 'jap\u00e1n', Portuguese: 'portug\u00e1l', Russian: 'orosz',
  Chinese: 'k\u00ednai', Korean: 'koreai',   Arabic: 'arab',
  Dutch: 'holland',    Polish: 'lengyel',     Turkish: 't\u00f6r\u00f6k',
  Romanian: 'rom\u00e1n', Croatian: 'horv\u00e1t', Serbian: 'szerb',
  Czech: 'cseh',       Greek: 'g\u00f6r\u00f6g', Swedish: 'sv\u00e9d',
  Norwegian: 'norv\u00e9g', Danish: 'd\u00e1n', Finnish: 'finn',
};

const LANG_ISO = {
  English: 'eng',    Hungarian: 'hun', French: 'fre',  German: 'ger',
  Spanish: 'spa',    Italian: 'ita',   Portuguese: 'por', Russian: 'rus',
  Japanese: 'jpn',   Chinese: 'chi',   Korean: 'kor',  Dutch: 'dut',
  Polish: 'pol',     Swedish: 'swe',   Norwegian: 'nor', Danish: 'dan',
  Finnish: 'fin',    Czech: 'cze',     Romanian: 'rum', Turkish: 'tur',
  Arabic: 'ara',     Hebrew: 'heb',    Greek: 'ell',   Ukrainian: 'ukr',
};

const engToHun = lang => LANG_HUN[lang] || '';
const langToISO639 = lang => LANG_ISO[lang] || 'eng';

// ── Subtitle HTML parser (feliratok.eu) ───────────────────────────────────────

function findFirstOf(str, chars, start) {
  for (let i = start; i < str.length; i++) {
    if (chars.includes(str[i])) return i;
  }
  return -1;
}

function parseSubtitleHTML(html) {
  const results = [];
  const TERMS = '"&\r\n';
  let pos = 0;

  while (true) {
    const fnevIdx = html.indexOf('fnev=', pos);
    if (fnevIdx === -1) break;
    const fnevStart = fnevIdx + 5;
    const fnevEnd = findFirstOf(html, TERMS, fnevStart);
    if (fnevEnd === -1) break;
    const filename = html.slice(fnevStart, fnevEnd);

    const idIdx = html.indexOf('felirat=', fnevEnd);
    if (idIdx === -1) break;
    const idStart = idIdx + 8;
    const idEnd = findFirstOf(html, TERMS, idStart);
    if (idEnd === -1) break;
    const subId = html.slice(idStart, idEnd);

    results.push({ subId, filename });
    pos = idEnd;
  }
  return results;
}

// ── Subtitle archive helpers ──────────────────────────────────────────────────

function walkDir(dir, depth = 0) {
  if (depth > 5) return [];
  const files = [];
  try {
    for (const entry of fs.readdirSync(dir)) {
      const full = path.join(dir, entry);
      try {
        const stat = fs.statSync(full);
        if (stat.isDirectory()) {
          files.push(...walkDir(full, depth + 1));
        } else if (/\.(srt|sub)$/i.test(entry)) {
          files.push(full);
        }
      } catch {}
    }
  } catch {}
  return files;
}

function findEpisodeSubtitle(extractDir, episode) {
  const files = walkDir(extractDir);
  if (!files.length) return '';

  const ep2 = String(episode).padStart(2, '0');
  const ep1 = String(episode);
  let best = '';
  let bestScore = -1;

  for (const f of files) {
    const lf = f.toLowerCase();
    let score = 0;
    if (lf.includes(`- ${ep2} -`))   score = 10;
    else if (lf.includes(`e${ep2}`)) score = 9;
    else if (lf.includes(`_${ep2}_`)) score = 8;
    else if (lf.includes(`.${ep2}.`)) score = 7;
    else if (lf.includes(`- ${ep1} -`)) score = 6;
    else if (lf.includes(`e${ep1}.`))   score = 5;

    if (score > bestScore) { bestScore = score; best = f; }
    else if (bestScore < 0) best = f;
  }
  return best;
}

function extractSubtitleArchive(archivePath, subId, episode) {
  const extractDir = `/tmp/imdbsub_${subId}/`;
  if (!walkDir(extractDir).length) {
    try { execSync(`rm -rf "${extractDir}" && mkdir -p "${extractDir}"`); } catch {}
    try {
      execSync(`unar -D -no-directory -force-overwrite "${archivePath}" -o "${extractDir}" > /dev/null 2>&1`);
    } catch {}
  }
  return findEpisodeSubtitle(extractDir, episode);
}

// ── Subtitle mux helper ───────────────────────────────────────────────────────

function muxSubtitleIntoVideo(outputBase) {
  if (!config.embedSubs) return;
  const videoPath = `${outputBase}.mp4`;
  const srtPath   = `${outputBase}.srt`;
  const tmpMux    = `${outputBase}_mux.mp4`;
  const muxCmd    = `ffmpeg -y -i "${videoPath}" -i "${srtPath}" -c:v copy -c:a copy -c:s mov_text ` +
                    `-metadata:s:s:0 language=${config.subLang} "${tmpMux}"`;
  console.log('[Subs] Muxing subtitle into video...');
  try {
    execSync(muxCmd, { stdio: 'inherit' });
    fs.unlinkSync(videoPath);
    fs.renameSync(tmpMux, videoPath);
    try { fs.unlinkSync(srtPath); } catch {}
    console.log(`[Subs] Embedded into: ${videoPath}`);
  } catch {
    console.error('[Subs] ffmpeg mux failed; keeping standalone .srt');
    try { fs.unlinkSync(tmpMux); } catch {}
  }
}

// ── TV subtitle downloader (feliratok.eu) ─────────────────────────────────────

async function downloadSubtitle(title, season, episode, outputBase) {
  if (config.noSubs) return;

  const hunLang = engToHun(config.subLang);
  console.log(`\n[Subs] Searching for ${config.subLang} subtitles on feliratok.eu...`);

  // 1. Resolve show ID via autoname
  const lookupTitle = stripYear(title);
  const autoResp = await fetchSubAPI(`action=autoname&nyelv=0&term=${encodeURIComponent(lookupTitle)}`);
  if (!autoResp) { console.error('[Subs] autoname request failed.'); return; }

  let autoData;
  try { autoData = JSON.parse(autoResp); } catch { console.error('[Subs] Failed to parse autoname response.'); return; }
  if (!Array.isArray(autoData) || !autoData.length) { console.error(`[Subs] No show ID found for "${title}".`); return; }
  if (autoData[0]?.ID === '-100x') { console.error('[Subs] Show not found on feliratok.eu.'); return; }

  // Pick highest numeric ID (most recently added entry)
  let showId = autoData[0].ID;
  for (const entry of autoData) {
    const id = entry.ID || '0';
    if (id !== '-100x' && parseInt(id) > parseInt(showId)) showId = id;
  }
  console.log(`[Subs] Show ID: ${showId}`);

  // 2. Search for subtitles via HTML
  let searchParams = `action=search&sid=${showId}`;
  if (season  > 0) searchParams += `&ev=${season}`;
  if (episode > 0) searchParams += `&epizod=${episode}`;
  if (hunLang) searchParams += `&nyelv=${encodeURIComponent(hunLang)}`;

  let html = await fetchSubAPI(searchParams);
  let results = parseSubtitleHTML(html);

  // Fallback: retry without language filter
  if (!results.length && hunLang) {
    console.log(`[Subs] No ${config.subLang} subtitle found; retrying without language filter...`);
    let fallbackParams = `action=search&sid=${showId}`;
    if (season  > 0) fallbackParams += `&ev=${season}`;
    if (episode > 0) fallbackParams += `&epizod=${episode}`;
    html = await fetchSubAPI(fallbackParams);
    results = parseSubtitleHTML(html);
  }

  if (!results.length) { console.error('[Subs] No subtitles found.'); return; }

  const { subId, filename: subFile } = results[0];
  console.log(`[Subs] Found: ${subFile} (ID ${subId})`);

  // 3. Download subtitle archive
  const tmpPath = `/tmp/imdbsub_${subId}.dl`;
  const dlURL   = `${SUB_BASE}?action=letolt&felirat=${subId}`;

  let needDownload = true;
  try { needDownload = fs.statSync(tmpPath).size < 100; } catch {}

  if (needDownload && !await downloadBinaryFile(dlURL, tmpPath)) {
    console.error('[Subs] Download failed.');
    return;
  }

  try {
    if (fs.statSync(tmpPath).size < 100) {
      console.error('[Subs] Downloaded archive is empty or invalid.');
      try { fs.unlinkSync(tmpPath); } catch {}
      return;
    }
  } catch {}

  // 4. Extract or move to final destination
  const ext = subFile.length > 4 ? subFile.slice(-4).toLowerCase() : '';

  if (ext === '.zip' || ext === '.rar') {
    const extracted = extractSubtitleArchive(tmpPath, subId, episode > 0 ? episode : 1);
    if (extracted) {
      const dest = `${outputBase}.srt`;
      try { fs.renameSync(extracted, dest); } catch {
        execSync(`cp "${extracted}" "${dest}"`);
        try { fs.unlinkSync(extracted); } catch {}
      }
      console.log(`[Subs] Saved: ${dest}`);
    } else {
      console.error('[Subs] No .srt/.sub found inside archive.');
      try { execSync(`rm -rf /tmp/imdbsub_${subId}/`); } catch {}
      try { fs.unlinkSync(tmpPath); } catch {}
      return;
    }
  } else {
    const dest = `${outputBase}${ext || '.srt'}`;
    try { fs.renameSync(tmpPath, dest); } catch {
      execSync(`cp "${tmpPath}" "${dest}"`);
    }
    console.log(`[Subs] Saved: ${dest}`);
    try { fs.unlinkSync(tmpPath); } catch {}
  }

  muxSubtitleIntoVideo(outputBase);
}

// ── Movie subtitle downloader (OpenSubtitles + wyzie.io) ─────────────────────

async function downloadSubtitleMovie(imdbId, outputBase) {
  if (config.noSubs) return;

  let imdbNum = imdbId.startsWith('tt') ? imdbId.slice(2) : imdbId;
  imdbNum = imdbNum.replace(/^0+/, '');

  const langCode = langToISO639(config.subLang);
  const searchURL = `https://rest.opensubtitles.org/search/imdbid-${imdbNum}/sublanguageid-${langCode}`;

  console.log(`\n[Subs] Searching OpenSubtitles for ${config.subLang} subtitles...`);
  const raw = await fetchOpenSubtitles(searchURL);
  if (!raw) { console.error('[Subs] OpenSubtitles request failed.'); return; }

  let results;
  try { results = JSON.parse(raw); } catch { console.error('[Subs] Failed to parse OpenSubtitles response.'); return; }
  if (!Array.isArray(results) || !results.length) { console.error('[Subs] No subtitles found on OpenSubtitles.'); return; }

  // Pick best result: prefer SubHD=1 and SubFromTrusted=1, then sort by download count
  let best = null;
  let bestScore = -1;
  for (const s of results) {
    const score = parseInt(s.SubDownloadsCnt || '0')
                + (s.SubHD === '1' ? 1000000 : 0)
                + (s.SubFromTrusted === '1' ? 500000 : 0);
    if (score > bestScore) { bestScore = score; best = s; }
  }

  const fileID  = best?.IDSubtitleFile || '';
  const dlLink  = best?.SubDownloadLink || '';
  const subFile = best?.SubFileName || 'subtitle';
  if (!fileID || !dlLink) { console.error('[Subs] Missing subtitle file info.'); return; }

  // Extract VRF hash from SubDownloadLink (e.g. "vrf-19cc0c55")
  let vrfHash = '';
  const vrfIdx = dlLink.indexOf('vrf-');
  if (vrfIdx !== -1) {
    const start = vrfIdx + 4;
    const end = dlLink.indexOf('/', start);
    vrfHash = dlLink.slice(start, end === -1 ? start + 8 : end);
  }

  console.log(`[Subs] Found: ${subFile}`);

  const tmpSrt = `/tmp/imdbsub_${fileID}.srt`;
  let downloaded = false;

  // Try wyzie.io first
  if (vrfHash) {
    const wyzieURL = `https://sub.wyzie.io/c/${vrfHash}/id/${fileID}?format=srt&encoding=UTF-8`;
    downloaded = await downloadWyzie(wyzieURL, tmpSrt);
    if (downloaded) {
      // Validate: wyzie.io may return a JSON error instead of SRT content
      try {
        const head = fs.readFileSync(tmpSrt).slice(0, 15).toString();
        if (!head || head[0] === '{' || head[0] === '[') {
          fs.unlinkSync(tmpSrt);
          downloaded = false;
        }
      } catch { downloaded = false; }
    }
  }

  // Fallback: download .gz directly from OpenSubtitles and decompress
  if (!downloaded) {
    const tmpGz = `/tmp/imdbsub_${fileID}.gz`;
    if (await downloadBinaryFile(dlLink, tmpGz)) {
      try {
        const decompressed = zlib.gunzipSync(fs.readFileSync(tmpGz));
        fs.writeFileSync(tmpSrt, decompressed);
        downloaded = true;
      } catch {}
      try { fs.unlinkSync(tmpGz); } catch {}
    }
  }

  if (!downloaded) { console.error('[Subs] Subtitle download failed.'); return; }

  const dest = `${outputBase}.srt`;
  try { fs.renameSync(tmpSrt, dest); } catch {
    execSync(`cp "${tmpSrt}" "${dest}"`);
    try { fs.unlinkSync(tmpSrt); } catch {}
  }
  console.log(`[Subs] Saved: ${dest}`);

  muxSubtitleIntoVideo(outputBase);
}

// ── Video downloader ──────────────────────────────────────────────────────────

function downloadStream(m3u8Url, outputPath) {
  const args = [
    '--user-agent', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0',
    '--referer', 'https://brightpathsignals.com/',
    '--downloader', 'ffmpeg',
    m3u8Url,
    '-o', outputPath,
  ];
  console.log('\nExecuting: yt-dlp', args.join(' '));
  spawnSync('yt-dlp', args, { stdio: 'inherit' });
}

// ── Content handlers ──────────────────────────────────────────────────────────

async function handleMovie(imdbId, title, streamUrls) {
  if (!streamUrls || !streamUrls.length) {
    console.error('No streams found for this movie.');
    return;
  }
  const base = `./${sanitizeFilename(title)}`;
  console.log(`\nFound Movie: ${title}`);
  console.log(`Downloading to ${base}.mp4...`);
  downloadStream(streamUrls[0], `${base}.mp4`);
  await downloadSubtitleMovie(imdbId, base);
}

async function handleShow(imdbId, title, epsData) {
  console.log(`\nFound TV Show: ${title}`);
  console.log('Available Seasons:');

  const seasons = Object.keys(epsData);
  for (const s of seasons) {
    console.log(`  Season ${s} (${epsData[s].length} episodes)`);
  }

  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const ask = q => new Promise(res => rl.question(q, res));

  console.log('\nOptions:\n  1. Download one specific episode\n  2. Download ALL episodes');
  const modeStr = await ask('Choose an option (1-2): ');
  const mode = parseInt(modeStr);
  const cleanTitle = sanitizeFilename(title);

  if (mode === 1) {
    const chosenSeason = await ask('Enter Season Number: ');
    const chosenEpStr  = await ask('Enter Episode Number: ');
    rl.close();
    const chosenEp = parseInt(chosenEpStr);

    const epUrl = `https://streamdata.vaplayer.ru/api.php?imdb=${imdbId}&type=tv&season=${chosenSeason}&episode=${chosenEp}`;
    const epRes = JSON.parse(stripToJSON(await fetchURL(epUrl)));
    const urls = epRes?.data?.stream_urls || [];

    if (urls.length) {
      const base = `./${cleanTitle}-S${chosenSeason}-E${chosenEp}`;
      downloadStream(urls[0], `${base}.mp4`);
      await downloadSubtitle(title, parseInt(chosenSeason), chosenEp, base);
    } else {
      console.error('Failed to find streams for that episode.');
    }

  } else if (mode === 2) {
    rl.close();
    console.log('\nStarting bulk download queue...');
    for (const seasonNum of seasons) {
      const epCount = epsData[seasonNum].length;
      for (let ep = 1; ep <= epCount; ep++) {
        console.log(`\n--- Fetching S${seasonNum}E${ep} ---`);
        const epUrl = `https://streamdata.vaplayer.ru/api.php?imdb=${imdbId}&type=tv&season=${seasonNum}&episode=${ep}`;
        try {
          const epRes = JSON.parse(stripToJSON(await fetchURL(epUrl)));
          const urls = epRes?.data?.stream_urls || [];
          if (urls.length) {
            const dir = `./${cleanTitle}/Season_${seasonNum}`;
            fs.mkdirSync(dir, { recursive: true });
            const base = `${dir}/${cleanTitle}-S${seasonNum}-E${ep}`;
            downloadStream(urls[0], `${base}.mp4`);
            await downloadSubtitle(title, parseInt(seasonNum), ep, base);
          }
        } catch {
          console.error(`Skipping S${seasonNum}E${ep} due to API parsing error.`);
        }
      }
    }
  } else {
    rl.close();
    console.error('Invalid option.');
  }
}

// ── Dependency check ──────────────────────────────────────────────────────────

function checkDependencies() {
  const isMac = os.platform() === 'darwin';
  const tools = [
    { cmd: 'unar',   brew: 'unar',   apt: 'unar'   },
    { cmd: 'yt-dlp', brew: 'yt-dlp', apt: 'yt-dlp' },
    { cmd: 'ffmpeg', brew: 'ffmpeg', apt: 'ffmpeg'  },
  ];

  let missing = false;
  for (const t of tools) {
    try { execSync(`command -v ${t.cmd}`, { stdio: 'ignore' }); }
    catch {
      if (!missing) { console.error('Missing required dependencies:'); missing = true; }
      console.error(isMac
        ? `  ${t.cmd}  →  brew install ${t.brew}`
        : `  ${t.cmd}  →  sudo apt install ${t.apt}`);
    }
  }
  return !missing;
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  program
    .name('imdbdownloader')
    .description('Download movies and TV shows by IMDB ID')
    .argument('<imdb_id>', 'IMDB ID (e.g. tt0480489)')
    .option('--no-subs', 'Skip subtitle download entirely')
    .option('--embed-subs', 'Mux subtitle track into the .mp4 using ffmpeg (removes .srt)')
    .option('--lang <language>', 'Subtitle language', 'English')
    .addHelpText('after', `
Examples:
  $ imdbdownloader tt0480489
  $ imdbdownloader tt0480489 --embed-subs
  $ imdbdownloader tt0480489 --lang English
  $ imdbdownloader tt0480489 --no-subs`)
    .parse();

  const [imdbId] = program.args;
  const opts = program.opts();

  // commander: --no-subs sets opts.subs === false; default is true
  config.noSubs    = opts.subs === false;
  config.embedSubs = opts.embedSubs || false;
  config.subLang   = opts.lang || 'English';

  if (!checkDependencies()) process.exit(1);

  console.log('Analyzing IMDB Media Signature...');
  const initialUrl = `https://streamdata.vaplayer.ru/api.php?imdb=${imdbId}&type=tv`;
  const rawJson = await fetchURL(initialUrl);

  if (!rawJson) {
    console.error('Failed to retrieve data from api endpoint.');
    process.exit(1);
  }

  const res = JSON.parse(stripToJSON(rawJson));
  const title = res?.data?.title;

  if (res?.data?.eps === false) {
    const movieUrl = `https://streamdata.vaplayer.ru/api.php?imdb=${imdbId}&type=movie`;
    const movieRes = JSON.parse(stripToJSON(await fetchURL(movieUrl)));
    await handleMovie(imdbId, title, movieRes?.data?.stream_urls);
  } else {
    await handleShow(imdbId, title, res?.data?.eps);
  }
}

main().catch(err => { console.error(err.message || err); process.exit(1); });
