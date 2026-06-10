#!/usr/bin/env node

'use strict';

const { execSync, spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const readline = require('readline');
const axios = require('axios');
const { program } = require('commander');

// ── Global config ─────────────────────────────────────────────────────────────

const config = {
  threads: 3,
  fragments: 8,
  apiKey: '',
  embedSubs: false,
  subLang: 'English',
};

const ANIAPI_BASE   = 'https://aniapi.kobosh.com';

// ── Download Management & UI ──────────────────────────────────────────────────

class DownloadManager {
  constructor(threads) {
    this.tasks = [];
    this.threads = threads;
    this.workerStatus = Array.from({ length: threads }, (_, i) => ({
      id: i + 1,
      status: 'Idle',
      progress: 0,
      currentTask: null,
      lastOutput: '',
    }));
    this.isBulk = false;
  }

  addTask(task) {
    this.tasks.push({
      ...task,
      downloaded: false,
      claimed: null,
      failed: false,
    });
  }

  claimTask(workerId) {
    const task = this.tasks.find(t => !t.claimed && !t.downloaded && !t.failed);
    if (task) {
      task.claimed = workerId;
      return task;
    }
    return null;
  }

  updateWorker(workerId, update) {
    const worker = this.workerStatus.find(w => w.id === workerId);
    if (worker) {
      Object.assign(worker, update);
      this.render();
    }
  }

  render() {
    if (!this.isBulk) return;

    const completed = this.tasks.filter(t => t.downloaded).length;
    const failed = this.tasks.filter(t => t.failed).length;
    const total = this.tasks.length;
    const processed = completed + failed;
    const percent = total > 0 ? Math.floor((processed / total) * 100) : 0;
    
    const terminalWidth = process.stdout.columns || 80;
    const failedText = failed > 0 ? `, ${failed} failed` : '';
    const statusText = ` ${percent}% (${processed}/${total} episodes${failedText})`;
    const prefix = "Total Progress: ";
    
    // Calculate remaining space for the bar (2 accounts for '[' and ']')
    const barWidth = Math.max(10, terminalWidth - prefix.length - statusText.length - 2);
    const filledWidth = total > 0 ? Math.floor((processed / total) * barWidth) : 0;
    const bar = '[' + '#'.repeat(filledWidth) + '-'.repeat(barWidth - filledWidth) + ']';

    // Move cursor up to overwrite previous lines (2 for total progress + 2 per thread)
    const lines = (this.workerStatus.length * 2) + 2;
    readline.cursorTo(process.stdout, 0);
    readline.moveCursor(process.stdout, 0, -lines);

    // Render Total Progress
    process.stdout.write(`\x1b[K${prefix}${bar}${statusText}\n\x1b[K\n`);

    for (const w of this.workerStatus) {
      const taskLabel = w.currentTask ? `S${w.currentTask.season}E${w.currentTask.episode}` : 'None';
      
      const statusLine = `Thread ${w.id}: ${taskLabel.padEnd(8)} | [${w.status}]`;
      process.stdout.write(`\x1b[K${statusLine.slice(0, terminalWidth)}\n`);
      const out = w.lastOutput || '';
      process.stdout.write(`\x1b[K  ${out.slice(0, terminalWidth - 4)}\n`);
    }
  }

  startBulk() {
    this.isBulk = true;
    // Prepare space for progress bars (2 for total progress + 2 per thread)
    for (let i = 0; i < (this.threads * 2) + 2; i++) process.stdout.write('\n');
    this.render();
  }
}

// ── AniAPI helpers ───────────────────────────────────────────────────────────

async function fetchAniApi(pathname) {
  try {
    const res = await axios.get(`${ANIAPI_BASE}${pathname}`, {
      headers: { 
        'x-api-key': config.apiKey,
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0'
      },
      timeout: 30000,
    });
    return res.data || null;
  } catch (e) {
    const msg = e?.response?.data?.error || e.message;
    throw new Error(msg || 'AniAPI request failed');
  }
}

// ── IMDB metadata (AniAPI /info) ─────────────────────────────────────────────

async function fetchImdbMetadata(imdbId) {
  try {
    const d = await fetchAniApi(`/info/${imdbId}`);
    return {
      title:         d.title || d.originalTitle || imdbId,
      originalTitle: d.originalTitle || d.title || imdbId,
      type:          d.mediaType || d.type || 'movie',
      genres:        d.genres || [],
      startYear:     d.year || null,
      episodes:      d.episodes || null,
      hasPrimaryStream: d.hasPrimaryStream !== false,
    };
  } catch (e) {
    console.error('[Meta] AniAPI lookup failed:', e.message);
    return {
      title: imdbId,
      originalTitle: imdbId,
      type: 'movie',
      genres: [],
      startYear: null,
      episodes: null,
      hasPrimaryStream: false,
    };
  }
}

function isShowType(type) {
  return /show|series|tv|mini|episode|special/i.test(type);
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function sanitizeFilename(name) {
  return name.replace(/ /g, '_').replace(/[^a-zA-Z0-9_\-]/g, '');
}

// ── Subtitle management ──────────────────────────────────────────────────────

async function handleSubtitles(imdbId, season, episode, videoPath, workerId = 0, manager = null) {
  if (!config.embedSubs) return;

  const log = (msg) => {
    if (manager && workerId > 0) {
      manager.updateWorker(workerId, { lastOutput: msg });
    } else {
      console.log(msg);
    }
  };

  try {
    const path = season 
      ? `/subtitles/show/${imdbId}/${season}/${episode}`
      : `/subtitles/movie/${imdbId}`;
    
    log(`[Subs] Fetching subtitles...`);
    const subs = await fetchAniApi(path);
    if (!subs || subs.length === 0) {
      log('[Subs] No subtitles found.');
      return;
    }

    // New API returns all candidates sorted by rating; prefer configured language then top item.
    const prefLang = (config.subLang || '').toLowerCase();
    const sub = subs.find(s => (s.language || '').toLowerCase() === prefLang) || subs[0];
    log(`[Subs] Downloading ${sub.language} subtitle...`);

    const subUrl = sub.url.startsWith('http') ? sub.url : `${ANIAPI_BASE}${sub.url}`;
    const subResponse = await axios.get(subUrl, { 
      headers: { 'x-api-key': config.apiKey },
      responseType: 'arraybuffer' 
    });

    const subExt = (() => {
      if (sub.format && /^[a-z0-9]+$/i.test(sub.format)) return `.${sub.format.toLowerCase()}`;
      if (sub.filename && sub.filename.includes('.')) return `.${sub.filename.split('.').pop().toLowerCase()}`;
      return '.srt';
    })();
    const subPath = videoPath.replace(/\.mp4$/, subExt);
    fs.writeFileSync(subPath, Buffer.from(subResponse.data));

    log(`[Subs] Embedding subtitle...`);
    const tempVideoPath = videoPath.replace(/\.mp4$/, '.temp.mp4');
    
    // Mux with ffmpeg: copy video/audio, add subtitle as mov_text
    const ffmpegArgs = [
      '-y',
      '-i', videoPath,
      '-i', subPath,
      '-c', 'copy',
      '-c:s', 'mov_text',
      '-metadata:s:s:0', `language=${sub.language.slice(0, 3).toLowerCase()}`,
      tempVideoPath
    ];

    await new Promise((resolve, reject) => {
      const child = spawn('ffmpeg', ffmpegArgs);
      child.on('close', (code) => {
        if (code === 0) resolve();
        else reject(new Error(`ffmpeg failed with code ${code}`));
      });
    });

    // Replace original with muxed version and cleanup
    fs.renameSync(tempVideoPath, videoPath);
    fs.unlinkSync(subPath);
    log('[Subs] Embedded successfully.');
  } catch (err) {
    log(`[Subs] Error: ${err.message}`);
  }
}

// ── Video downloader ──────────────────────────────────────────────────────────

async function downloadStream(m3u8Url, outputPath, extraHeaders = {}, onProgress = null, fragments = 8, onOutput = null) {
  const userAgent = extraHeaders['User-Agent']
    || 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0';
  const args = [
    '-f', 'bestvideo+bestaudio/best',
    '--format-sort', 'res,quality',
    '--user-agent', userAgent,
    '--concurrent-fragments', String(fragments),
    '--extractor-args', 'generic:impersonate',
    '--newline',
    m3u8Url,
    '-o', outputPath,
  ];

  if (extraHeaders['Referer']) {
    args.unshift(extraHeaders['Referer']);
    args.unshift('--referer');
  }

  for (const [key, value] of Object.entries(extraHeaders)) {
    if (key === 'User-Agent' || key === 'Referer') continue;
    args.push('--add-header', `${key}:${value}`);
  }

  const maxRetries = 3;
  let retries = 0;

  while (retries <= maxRetries) {
    try {
      await new Promise((resolve, reject) => {
        const child = spawn('yt-dlp', args);
        let linesInitialized = false;

        child.stdout.on('data', (data) => {
          const text = data.toString();
          const lines = text.split('\n').filter(l => l.trim());
          if (lines.length === 0) return;
          const lastLine = lines[lines.length - 1];

          if (onOutput) {
            onOutput(lastLine);
          } else {
            if (!linesInitialized) {
              process.stdout.write('\n');
              linesInitialized = true;
            }
            readline.moveCursor(process.stdout, 0, -1);
            process.stdout.write(`\r\x1b[KStatus: Downloading...\n\x1b[K${lastLine.slice(0, (process.stdout.columns || 80) - 1)}`);
          }

          if (onProgress) {
            const match = /\[download\]\s+(\d+\.\d+)%/.exec(lastLine);
            if (match) {
              onProgress(parseFloat(match[1]));
            }
          }
        });

        child.on('close', (code) => {
          if (!onOutput && linesInitialized) process.stdout.write('\n');
          if (code === 0) resolve();
          else reject(new Error(`yt-dlp failed with code ${code}`));
        });
      });
      return; // Success, exit the retry loop
    } catch (err) {
      retries++;
      if (retries > maxRetries) throw err;
      const retryMsg = `yt-dlp failed, retrying in 5s (${retries}/${maxRetries})...`;
      if (onOutput) onOutput(retryMsg);
      else console.log(`\n${retryMsg}`);
      await new Promise(r => setTimeout(r, 5000));
    }
  }
}

// ── Content handlers ──────────────────────────────────────────────────────────

async function downloadWorker(workerId, manager, streamSourceFn) {
  while (true) {
    const task = manager.claimTask(workerId);
    if (!task) break;

    manager.updateWorker(workerId, {
      status: 'Downloading',
      progress: 0,
      currentTask: task,
    });

    try {
      const { season, episode, baseDir, fileNameBase, extraHeaders, imdbId } = task;
      const m3u8 = await streamSourceFn(task);
      if (!m3u8) throw new Error('No stream URL');

      fs.mkdirSync(baseDir, { recursive: true });
      const outputPath = `${fileNameBase}.mp4`;

      await downloadStream(m3u8, outputPath, extraHeaders || {}, (p) => {
        manager.updateWorker(workerId, { progress: p });
      }, config.fragments, (line) => {
        manager.updateWorker(workerId, { lastOutput: line });
      });

      await handleSubtitles(imdbId, season, episode, outputPath, workerId, manager);

      task.downloaded = true;
      manager.updateWorker(workerId, { status: 'Done', progress: 100 });
    } catch (err) {
      task.failed = true;
      manager.updateWorker(workerId, { status: `Error: ${err.message.slice(0, 15)}`, progress: 0 });
    }
  }
  manager.updateWorker(workerId, { status: 'Finished', currentTask: null });
}

// ── Content handlers ──────────────────────────────────────────────────────────

async function handleMovie(imdbId, title) {
  let movieData;
  try {
    movieData = await fetchAniApi(`/download/movie/${imdbId}`);
  } catch {
    console.error('No streams found for this movie.');
    return;
  }

  const streamUrl = movieData?.streamUrl || '';
  const headers = movieData?.headers || {};
  if (!streamUrl) {
    console.error('No streams found for this movie.');
    return;
  }

  const base = `./${sanitizeFilename(title)}`;
  console.log(`\nFound Movie: ${title}`);
  console.log(`Downloading to ${base}.mp4...`);
  
  const outputPath = `${base}.mp4`;
  await downloadStream(streamUrl, outputPath, headers, null, config.fragments);
  await handleSubtitles(imdbId, null, null, outputPath);
  console.log('\nDownload complete.');
}

async function handleShow(imdbId, title, _originalTitle, epsData) {
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const ask = q => new Promise(res => rl.question(q, res));

  if (epsData && typeof epsData === 'object' && Object.keys(epsData).length) {
    console.log(`\nFound TV Show: ${title}`);
    console.log('Available Seasons:');
    const seasons = Object.keys(epsData);
    for (const s of seasons) {
      const count = Array.isArray(epsData[s]) ? epsData[s].length : epsData[s];
      console.log(`  Season ${s} (${count} episodes)`);
    }

    console.log('\nOptions:\n  1. Download one specific episode\n  2. Download ALL episodes');
    const modeStr = await ask('Choose an option (1-2): ');
    const mode = parseInt(modeStr);
    const cleanTitle = sanitizeFilename(title);

    if (mode === 1) {
      const chosenSeason = await ask('Enter Season Number: ');
      const chosenEpStr  = await ask('Enter Episode Number: ');
      rl.close();
      const chosenEp = parseInt(chosenEpStr);

      try {
        const epRes = await fetchAniApi(`/download/show/${imdbId}/${chosenSeason}/${chosenEp}`);
        const streamUrl = epRes?.streamUrl || '';
        const headers = epRes?.headers || {};
        if (streamUrl) {
          const base = `./${cleanTitle}-S${chosenSeason}-E${chosenEp}`;
          const outputPath = `${base}.mp4`;
          console.log(`\nDownloading S${chosenSeason}E${chosenEp}...`);
          await downloadStream(streamUrl, outputPath, headers, null, config.fragments);
          await handleSubtitles(imdbId, chosenSeason, chosenEp, outputPath);
          console.log('\nDownload complete.');
        } else {
          console.error('No stream found via primary source.');
        }
      } catch {
        console.error('Primary source failed for that episode.');
      }
    } else if (mode === 2) {
      rl.close();
      const manager = new DownloadManager(config.threads);
      for (const seasonNum of seasons) {
        const epList = epsData[seasonNum];
        const epCount = Array.isArray(epList) ? epList.length : parseInt(epList) || 0;
        for (let ep = 1; ep <= epCount; ep++) {
          manager.addTask({
            season: seasonNum,
            episode: ep,
            baseDir: `./${cleanTitle}/Season_${seasonNum}`,
            fileNameBase: `./${cleanTitle}/Season_${seasonNum}/${cleanTitle}-S${seasonNum}-E${ep}`,
            imdbId,
          });
        }
      }

      console.log(`\nStarting bulk download (${manager.tasks.length} episodes) with ${config.threads} threads...`);
      manager.startBulk();

      const sourceFn = async (task) => {
        const res = await fetchAniApi(`/download/show/${task.imdbId}/${task.season}/${task.episode}`);
        task.extraHeaders = res?.headers || {};
        return res?.streamUrl || null;
      };

      const workers = Array.from({ length: config.threads }, (_, i) => 
        downloadWorker(i + 1, manager, sourceFn)
      );
      await Promise.all(workers);
      
      const failedCount = manager.tasks.filter(t => t.failed).length;
      if (failedCount > 0) {
        console.log(`\nNot all eps are downloaded and they need to run the command again`);
      } else {
        console.log('\nAll downloads completed.');
      }
    } else {
      rl.close();
      console.error('Invalid option.');
    }
    return;
  }

  console.log(`\nFound TV Show: ${title}`);
  console.log('[Info] AniAPI did not return episode metadata. Downloading a single episode only.');
  const cleanTitle = sanitizeFilename(title);
  const chosenSeason = await ask('Enter Season Number: ');
  const chosenEpStr  = await ask('Enter Episode Number: ');
  rl.close();
  const chosenEp = parseInt(chosenEpStr);

  try {
    const epRes = await fetchAniApi(`/download/show/${imdbId}/${chosenSeason}/${chosenEp}`);
    const streamUrl = epRes?.streamUrl || '';
    const headers = epRes?.headers || {};
    if (!streamUrl) {
      console.error('No stream found for that episode.');
      return;
    }
    const base = `./${cleanTitle}-S${chosenSeason}-E${chosenEp}`;
    const outputPath = `${base}.mp4`;
    console.log(`\nDownloading S${chosenSeason}E${chosenEp}...`);
    await downloadStream(streamUrl, outputPath, headers, null, config.fragments);
    await handleSubtitles(imdbId, chosenSeason, chosenEp, outputPath);
    console.log('\nDownload complete.');
  } catch (err) {
    console.error('AniAPI episode download failed:', err.message);
  }
}

// ── Dependency check ──────────────────────────────────────────────────────────

function checkDependencies() {
  const isMac = os.platform() === 'darwin';
  const tools = [
    { cmd: 'yt-dlp', brew: 'yt-dlp', apt: 'yt-dlp' },
    { cmd: 'ffmpeg', brew: 'ffmpeg', apt: 'ffmpeg' },
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
    .option('--key <apikey>', 'AniAPI key (falls back to ANIAPI_TOKEN env var)')
    .option('-t, --threads <number>', 'Number of concurrent downloads (shows only)', '3')
    .option('--concurrent-fragments <number>', 'Number of concurrent fragments per download', '8')
    .option('--embed-subs', 'Automatically download and embed subtitles', false)
    .option('--sub-lang <lang>', 'Preferred subtitle language', 'English')
    .addHelpText('after', `
Examples:
  $ imdbdownloader tt0480489 --embed-subs
  $ node downloader.js tt0480489 --key YOUR_API_KEY --embed-subs --sub-lang Hungarian
  $ node downloader.js tt0480489 --threads 5
  $ ANIAPI_TOKEN=YOUR_API_KEY node downloader.js tt0480489

Note: when using "npm start", pass flags after "--":
  $ npm start -- tt0480489 --key YOUR_API_KEY`)
    .parse();

  const [imdbId] = program.args;
  const opts = program.opts();

  config.threads   = parseInt(opts.threads) || 3;
  config.fragments = parseInt(opts.concurrentFragments) || 8;
  config.apiKey    = (opts.key || process.env.ANIAPI_TOKEN || '').toString().trim().replace(/['"]/g, '');
  config.embedSubs = !!opts.embedSubs;
  config.subLang   = opts.subLang || 'English';

  if (!config.apiKey) {
    console.error('Error: API key required. Contact @kobosh_com on telegram/@kobosh.com on discord for a api key');
    process.exit(1);
  }

  if (!checkDependencies()) process.exit(1);

  console.log('Analyzing IMDB Media Signature...');

  // 1. Fetch metadata from AniAPI
  const meta = await fetchImdbMetadata(imdbId);
  console.log(`\nTitle: ${meta.title} (${meta.type})`);

  if (!isShowType(meta.type)) {
    await handleMovie(imdbId, meta.title);
  } else {
    await handleShow(imdbId, meta.title, meta.originalTitle, meta.episodes ?? null);
  }
}

main().catch(err => { console.error(err.message || err); process.exit(1); });
