#include <iostream>
#include <string>
#include <vector>
#include <cstdlib>
#include <sstream>
#include <memory>
#include <algorithm>
#include <cstdio>
#include <curl/curl.h>
#include <nlohmann/json.hpp>

using json = nlohmann::json;

// ── Global flags ─────────────────────────────────────────────────────────────
bool g_noSubs = false;
bool g_embedSubs = false;
std::string g_subLang = "English"; // preferred subtitle language (English name)

// ── Generic write-to-string callback ─────────────────────────────────────────
size_t WriteCallback(void* contents, size_t size, size_t nmemb, void* userp) {
    ((std::string*)userp)->append((char*)contents, size * nmemb);
    return size * nmemb;
}

// Strip leading non-JSON content (e.g. PHP warnings) — find first '{' or '['
std::string stripToJSON(const std::string& s) {
    size_t p = s.find_first_of("{[");
    if (p == std::string::npos) return s;
    return s.substr(p);
}

// ── Write-to-FILE callback (for binary subtitle downloads) ───────────────────
size_t WriteFileCallback(void* contents, size_t size, size_t nmemb, void* userp) {
    return fwrite(contents, size, nmemb, (FILE*)userp);
}

// ── Stream downloader (video API) ────────────────────────────────────────────
std::string fetchURL(const std::string& url) {
    CURL* curl;
    CURLcode res;
    std::string readBuffer;

    curl = curl_easy_init();
    if (curl) {
        struct curl_slist* headers = NULL;
        headers = curl_slist_append(headers, "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0");
        headers = curl_slist_append(headers, "Referer: https://brightpathsignals.com/");

        curl_easy_setopt(curl, CURLOPT_URL, url.c_str());
        curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
        curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, WriteCallback);
        curl_easy_setopt(curl, CURLOPT_WRITEDATA, &readBuffer);
        curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);

        res = curl_easy_perform(curl);
        curl_slist_free_all(headers);
        curl_easy_cleanup(curl);

        if (res != CURLE_OK) {
            std::cerr << "CURL Error: " << curl_easy_strerror(res) << std::endl;
        }
    }
    return readBuffer;
}

// ── Subtitle API helpers (feliratok.eu) ───────────────────────────────────────
const std::string SUB_BASE = "https://feliratok.eu/index.php";

std::string urlEncode(const std::string& value) {
    CURL* curl = curl_easy_init();
    if (!curl) return value;
    char* enc = curl_easy_escape(curl, value.c_str(), (int)value.size());
    std::string result(enc);
    curl_free(enc);
    curl_easy_cleanup(curl);
    return result;
}

std::string fetchSubAPI(const std::string& query_params) {
    std::string url = SUB_BASE + "?" + query_params;
    CURL* curl = curl_easy_init();
    std::string buffer;
    if (curl) {
        struct curl_slist* headers = NULL;
        headers = curl_slist_append(headers, "User-Agent: xbmc subtitle plugin");
        curl_easy_setopt(curl, CURLOPT_URL, url.c_str());
        curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
        curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, WriteCallback);
        curl_easy_setopt(curl, CURLOPT_WRITEDATA, &buffer);
        curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
        curl_easy_perform(curl);
        curl_slist_free_all(headers);
        curl_easy_cleanup(curl);
    }
    return buffer;
}

bool downloadBinaryFile(const std::string& url, const std::string& filepath) {
    CURL* curl = curl_easy_init();
    if (!curl) return false;
    FILE* fp = fopen(filepath.c_str(), "wb");
    if (!fp) { curl_easy_cleanup(curl); return false; }
    struct curl_slist* headers = NULL;
    headers = curl_slist_append(headers, "User-Agent: xbmc subtitle plugin");
    curl_easy_setopt(curl, CURLOPT_URL, url.c_str());
    curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, WriteFileCallback);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, fp);
    curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
    CURLcode res = curl_easy_perform(curl);
    fclose(fp);
    curl_slist_free_all(headers);
    curl_easy_cleanup(curl);
    return res == CURLE_OK;
}

// Map English language name → Hungarian (feliratok.eu uses Hungarian lang keys)
static const std::vector<std::pair<std::string,std::string>> LANG_MAP = {
    {"English","angol"}, {"Hungarian","magyar"}, {"Spanish","spanyol"},
    {"French","francia"}, {"German","német"}, {"Italian","olasz"},
    {"Japanese","japán"}, {"Portuguese","portugál"}, {"Russian","orosz"},
    {"Chinese","kínai"}, {"Korean","koreai"}, {"Arabic","arab"},
    {"Dutch","holland"}, {"Polish","lengyel"}, {"Turkish","török"},
    {"Romanian","román"}, {"Croatian","horvát"}, {"Serbian","szerb"},
    {"Czech","cseh"}, {"Greek","görög"}, {"Swedish","svéd"},
    {"Norwegian","norvég"}, {"Danish","dán"}, {"Finnish","finn"},
};

std::string engToHun(const std::string& eng) {
    for (auto& p : LANG_MAP)
        if (p.first == eng) return p.second;
    return "";
}

// Parse the HTML from action=search and return {feliratID, filename} pairs.
// feliratok.eu returns HTML rather than JSON from this endpoint (action=xbmc
// is Cloudflare-blocked), so we scrape the download links directly.
std::vector<std::pair<std::string,std::string>> parseSubtitleHTML(const std::string& html) {
    std::vector<std::pair<std::string,std::string>> results;
    // Each download link looks like:
    //   href="/index.php?action=letolt&fnev=Title.S01.rar&felirat=12345"
    size_t pos = 0;
    while (true) {
        size_t fnev_pos = html.find("fnev=", pos);
        if (fnev_pos == std::string::npos) break;
        size_t fnev_start = fnev_pos + 5;
        size_t fnev_end   = html.find_first_of("\"&\r\n", fnev_start);
        if (fnev_end == std::string::npos) break;
        std::string filename = html.substr(fnev_start, fnev_end - fnev_start);

        size_t id_pos = html.find("felirat=", fnev_end);
        if (id_pos == std::string::npos) break;
        size_t id_start = id_pos + 8;
        size_t id_end   = html.find_first_of("\"&\r\n", id_start);
        if (id_end == std::string::npos) break;
        std::string subId = html.substr(id_start, id_end - id_start);

        results.push_back({subId, filename});
        pos = id_end;
    }
    return results;
}

// After extracting an archive that may be a season pack, find the .srt/.sub
// for the specific episode number inside extractDir.
std::string findEpisodeSubtitle(const std::string& extractDir, int episode) {
    // Build a shell command that lists candidates and scores them:
    // 1. Prefer exact episode match (e.g. "- 02 -", "E02", "_02_", " 2 ")
    // 2. Fall back to first .srt/.sub
    char epBuf[16];
    snprintf(epBuf, sizeof(epBuf), "%02d", episode);
    std::string ep2  = epBuf;                    // "02"
    std::string ep1  = std::to_string(episode);  // "2"

    // List all subtitle files in the dir
    std::string listCmd = "find \"" + extractDir + "\" -maxdepth 2 \\( -name '*.srt' -o -name '*.sub' \\) 2>/dev/null";
    FILE* pipe = popen(listCmd.c_str(), "r");
    if (!pipe) return "";

    std::vector<std::string> files;
    char buf[1024];
    while (fgets(buf, sizeof(buf), pipe)) {
        std::string f(buf);
        if (!f.empty() && f.back() == '\n') f.pop_back();
        files.push_back(f);
    }
    pclose(pipe);

    // Score each file by how well it matches the episode number
    std::string best;
    int bestScore = -1;
    for (auto& f : files) {
        // Lowercase filename for matching
        std::string lf = f;
        std::transform(lf.begin(), lf.end(), lf.begin(), ::tolower);
        int score = 0;
        // Strong match: "- 02 -" or "e02" or "_02_" or ".02."
        if (lf.find("- " + ep2 + " -") != std::string::npos) score = 10;
        else if (lf.find("e" + ep2) != std::string::npos)     score = 9;
        else if (lf.find("_" + ep2 + "_") != std::string::npos) score = 8;
        else if (lf.find("." + ep2 + ".") != std::string::npos) score = 7;
        else if (lf.find("- " + ep1 + " -") != std::string::npos) score = 6;
        else if (lf.find("e" + ep1 + ".") != std::string::npos)    score = 5;
        // Non-zero score means it's a candidate
        if (score > bestScore) { bestScore = score; best = f; }
        else if (bestScore < 0) best = f; // fallback: take the first one
    }
    return best;
}

// Extract a subtitle archive (zip/rar) and return path to the episode's .srt
std::string extractSubtitleArchive(const std::string& archivePath, const std::string& subId, int episode) {
    std::string extractDir = "/tmp/imdbsub_" + subId + "/";
    std::system(("rm -rf \"" + extractDir + "\" && mkdir -p \"" + extractDir + "\"").c_str());

    // unar (The Unarchiver) handles RAR3/RAR5 and zip better than p7zip on macOS
    std::string extractCmd = "unar -D -force-overwrite \"" + archivePath + "\" -o \"" + extractDir + "\" >/dev/null 2>&1";
    int rc = std::system(extractCmd.c_str());
    if (rc != 0) {
        // fallback to unzip for plain zip files
        std::system(("unzip -o -j \"" + archivePath + "\" '*.srt' '*.sub' -d \"" + extractDir + "\" >/dev/null 2>&1").c_str());
    }

    return findEpisodeSubtitle(extractDir, episode);
}

// Strip trailing year "(YYYY)" or " YYYY" and trailing punctuation from a title
std::string stripYear(const std::string& title) {
    std::string t = title;
    // Strip " YYYY" at end
    if (t.size() >= 5) {
        std::string end4 = t.substr(t.size() - 4);
        if (t[t.size() - 5] == ' ' && std::all_of(end4.begin(), end4.end(), ::isdigit))
            t = t.substr(0, t.size() - 5);
        // Strip " (YYYY)" at end
        else if (t.size() >= 7 && t.back() == ')') {
            size_t p = t.rfind(" (");
            if (p != std::string::npos && std::all_of(t.begin() + p + 2, t.end() - 1, ::isdigit))
                t = t.substr(0, p);
        }
    }
    // Strip trailing non-alphanumeric chars (e.g. trailing period in "Your Name.")
    while (!t.empty() && !std::isalnum((unsigned char)t.back()))
        t.pop_back();
    return t;
}

// After downloading a video to `outputBase`.mp4, fetch & save its subtitle.
// For TV: pass season ≥ 1 and episode ≥ 1.  For movies: pass season=0, episode=0.
void downloadSubtitle(const std::string& title, int season, int episode, const std::string& outputBase) {
    if (g_noSubs) return;

    std::string hunLang = engToHun(g_subLang);
    std::cout << "\n[Subs] Searching for " << g_subLang << " subtitles on feliratok.eu..." << std::endl;

    // 1. Resolve show/movie ID via autoname (this endpoint is not Cloudflare-blocked)
    //    Strip the year from the title so feliratok.eu can find it (e.g. "Elfen Lied 2004" → "Elfen Lied")
    std::string lookupTitle = stripYear(title);
    std::string autoResp = fetchSubAPI("action=autoname&nyelv=0&term=" + urlEncode(lookupTitle));
    if (autoResp.empty()) { std::cerr << "[Subs] autoname request failed.\n"; return; }

    json autoData;
    try { autoData = json::parse(autoResp); } catch (...) { std::cerr << "[Subs] Failed to parse autoname response.\n"; return; }
    if (!autoData.is_array() || autoData.empty()) { std::cerr << "[Subs] No show ID found for \"" << title << "\".\n"; return; }

    // Check for no-result sentinel "-100x"
    if (autoData[0].value("ID", "") == "-100x") { std::cerr << "[Subs] Show not found on feliratok.eu.\n"; return; }

    // Pick highest numeric ID (most recently added entry)
    std::string showId = autoData[0]["ID"].get<std::string>();
    for (auto& entry : autoData) {
        std::string id = entry.value("ID", "0");
        if (id != "-100x" && std::stoi(id) > std::stoi(showId))
            showId = id;
    }
    std::cout << "[Subs] Show ID: " << showId << std::endl;

    // 2. Search for subtitles via HTML (action=xbmc is Cloudflare-blocked; action=search returns HTML)
    std::string searchParams = "action=search&sid=" + showId;
    if (season  > 0) searchParams += "&ev=" + std::to_string(season);
    if (episode > 0) searchParams += "&epizod=" + std::to_string(episode);
    if (!hunLang.empty()) searchParams += "&nyelv=" + urlEncode(hunLang);

    std::string html = fetchSubAPI(searchParams);
    auto results = parseSubtitleHTML(html);

    // Fallback: retry without language filter if nothing found
    if (results.empty() && !hunLang.empty()) {
        std::cout << "[Subs] No " << g_subLang << " subtitle found; retrying without language filter..." << std::endl;
        std::string fallbackParams = "action=search&sid=" + showId;
        if (season  > 0) fallbackParams += "&ev=" + std::to_string(season);
        if (episode > 0) fallbackParams += "&epizod=" + std::to_string(episode);
        html = fetchSubAPI(fallbackParams);
        results = parseSubtitleHTML(html);
    }

    if (results.empty()) { std::cerr << "[Subs] No subtitles found.\n"; return; }

    auto& [subId, subFile] = results[0];
    std::cout << "[Subs] Found: " << subFile << " (ID " << subId << ")" << std::endl;

    // 3. Download subtitle archive to /tmp (spaces in filename are safe here)
    std::string safeName = subId + ".dl";
    std::string tmpPath  = "/tmp/imdbsub_" + safeName;
    std::string dlURL    = SUB_BASE + "?action=letolt&felirat=" + subId;
    if (!downloadBinaryFile(dlURL, tmpPath)) { std::cerr << "[Subs] Download failed.\n"; return; }

    // 4. Extract or move to final destination
    std::string ext = subFile.size() > 4 ? subFile.substr(subFile.size() - 4) : "";
    std::transform(ext.begin(), ext.end(), ext.begin(), ::tolower);

    if (ext == ".zip" || ext == ".rar") {
        std::string extracted = extractSubtitleArchive(tmpPath, subId, episode > 0 ? episode : 1);
        if (!extracted.empty()) {
            std::string dest = outputBase + ".srt";
            // rename may fail across filesystems; fall back to copy+remove
            if (std::rename(extracted.c_str(), dest.c_str()) != 0) {
                std::system(("cp \"" + extracted + "\" \"" + dest + "\"").c_str());
                std::remove(extracted.c_str());
            }
            std::cout << "[Subs] Saved: " << dest << std::endl;
        } else {
            std::cerr << "[Subs] No .srt/.sub found inside archive.\n";
        }
        std::system(("rm -rf /tmp/imdbsub_" + subId + "/").c_str());
    } else {
        // Plain .srt / .sub — just move it
        std::string dest = outputBase + (ext.empty() ? ".srt" : ext);
        if (std::rename(tmpPath.c_str(), dest.c_str()) != 0)
            std::system(("cp \"" + tmpPath + "\" \"" + dest + "\"").c_str());
        std::cout << "[Subs] Saved: " << dest << std::endl;
        std::remove(tmpPath.c_str());
        return;
    }

    std::remove(tmpPath.c_str());

    // 5. Optionally mux subtitle into the video with ffmpeg
    if (g_embedSubs) {
        std::string videoPath = outputBase + ".mp4";
        std::string srtPath   = outputBase + ".srt";
        std::string tmpMux    = outputBase + "_mux.mp4";
        // Soft-subtitle mux: copy all streams, add subtitle track as mov_text
        std::string muxCmd = "ffmpeg -y -i \"" + videoPath + "\" -i \"" + srtPath + "\""
                             " -c:v copy -c:a copy -c:s mov_text"
                             " -metadata:s:s:0 language=" + g_subLang +
                             " \"" + tmpMux + "\" 2>&1";
        std::cout << "[Subs] Muxing subtitle into video..." << std::endl;
        int rc = std::system(muxCmd.c_str());
        if (rc == 0) {
            std::remove(videoPath.c_str());
            if (std::rename(tmpMux.c_str(), videoPath.c_str()) != 0)
                std::system(("mv \"" + tmpMux + "\" \"" + videoPath + "\"").c_str());
            std::remove(srtPath.c_str());
            std::cout << "[Subs] Embedded into: " << videoPath << std::endl;
        } else {
            std::cerr << "[Subs] ffmpeg mux failed; keeping standalone .srt\n";
            std::remove(tmpMux.c_str());
        }
    }
}

std::string sanitizeFilename(std::string name) {
    std::replace(name.begin(), name.end(), ' ', '_');
    name.erase(std::remove_if(name.begin(), name.end(), [](char c) {
        return !(std::isalnum(c) || c == '_' || c == '-');
    }), name.end());
    return name;
}

void downloadStream(const std::string& m3u8_url, const std::string& output_path) {
    std::string cmd = "yt-dlp --user-agent \"Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0\" "
                      "--referer \"https://brightpathsignals.com/\" "
                      "--downloader ffmpeg "
                      "\"" + m3u8_url + "\" -o \"" + output_path + "\"";

    std::cout << "\nExecuting: " << cmd << std::endl;
    std::system(cmd.c_str());
}

void handleMovie(const std::string& title, const json& stream_urls) {
    if (stream_urls.empty()) {
        std::cerr << "No streams found for this movie." << std::endl;
        return;
    }
    std::string base = "./" + sanitizeFilename(title);
    std::string filename = base + ".mp4";
    std::cout << "\nFound Movie: " << title << std::endl;
    std::cout << "Downloading to " << filename << "..." << std::endl;
    downloadStream(stream_urls[0].get<std::string>(), filename);
    downloadSubtitle(title, 0, 0, base);
}

void handleShow(const std::string& imdb_id, const std::string& title, const json& eps_data) {
    std::cout << "\nFound TV Show: " << title << std::endl;
    std::cout << "Available Seasons:" << std::endl;

    std::vector<std::string> seasons;
    for (auto& el : eps_data.items()) {
        seasons.push_back(el.key());
        std::cout << "  Season " << el.key() << " (" << el.value().size() << " episodes)" << std::endl;
    }

    std::cout << "\nOptions:\n  1. Download one specific episode\n  2. Download ALL episodes\nChoose an option (1-2): ";
    int mode;
    std::cin >> mode;

    std::string cleanTitle = sanitizeFilename(title);

    if (mode == 1) {
        std::string chosen_season;
        int chosen_ep;
        std::cout << "Enter Season Number: ";
        std::cin >> chosen_season;
        std::cout << "Enter Episode Number: ";
        std::cin >> chosen_ep;

        std::string ep_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=tv&season=" + chosen_season + "&episode=" + std::to_string(chosen_ep);
        json ep_res = json::parse(stripToJSON(fetchURL(ep_url)));
        auto urls = ep_res["data"]["stream_urls"];

        if (!urls.empty()) {
            std::string base = "./" + cleanTitle + "-S" + chosen_season + "-E" + std::to_string(chosen_ep);
            downloadStream(urls[0].get<std::string>(), base + ".mp4");
            downloadSubtitle(title, std::stoi(chosen_season), chosen_ep, base);
        } else {
            std::cerr << "Failed to find streams for that episode." << std::endl;
        }

    } else if (mode == 2) {
        std::cout << "\nStarting bulk download queue..." << std::endl;
        for (const auto& season_num : seasons) {
            int ep_count = eps_data[season_num].size();
            for (int ep = 1; ep <= ep_count; ++ep) {
                std::cout << "\n--- Fetching S" << season_num << "E" << ep << " ---" << std::endl;
                std::string ep_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=tv&season=" + season_num + "&episode=" + std::to_string(ep);

                try {
                    json ep_res = json::parse(stripToJSON(fetchURL(ep_url)));
                    auto urls = ep_res["data"]["stream_urls"];
                    if (!urls.empty()) {
                        std::string dir_cmd = "mkdir -p \"./" + cleanTitle + "/Season_" + season_num + "\"";
                        std::system(dir_cmd.c_str());

                        std::string base = "./" + cleanTitle + "/Season_" + season_num + "/" + cleanTitle + "-S" + season_num + "-E" + std::to_string(ep);
                        downloadStream(urls[0].get<std::string>(), base + ".mp4");
                        downloadSubtitle(title, std::stoi(season_num), ep, base);
                    }
                } catch (...) {
                    std::cerr << "Skipping S" << season_num << "E" << ep << " due to API parsing error." << std::endl;
                }
            }
        }
    }
}

int main(int argc, char* argv[]) {
    if (argc < 2) {
        std::cerr << "Usage: " << argv[0] << " <IMDB_ID> [--no-subs] [--lang <Language>]\n"
                  << "  e.g.: " << argv[0] << " tt0480489\n"
                  << "        " << argv[0] << " tt0480489 --lang English\n"
                  << "        " << argv[0] << " tt0480489 --no-subs\n";
        return 1;
    }

    std::string imdb_id = argv[1];

    if (imdb_id == "--help" || imdb_id == "-h") {
        std::cout << "Usage: " << argv[0] << " <IMDB_ID> [--no-subs] [--embed-subs] [--lang <Language>]\n"
                  << "  e.g.: " << argv[0] << " tt0480489\n"
                  << "        " << argv[0] << " tt0480489 --embed-subs\n"
                  << "        " << argv[0] << " tt0480489 --lang English\n"
                  << "        " << argv[0] << " tt0480489 --no-subs\n"
                  << "\n  --embed-subs  Mux subtitle track into the .mp4 using ffmpeg (removes .srt)\n"
                  << "  --no-subs     Skip subtitle download entirely\n"
                  << "  --lang <L>    Subtitle language (default: English)\n";
        return 0;
    }

    for (int i = 2; i < argc; ++i) {
        std::string arg = argv[i];
        if (arg == "--no-subs") {
            g_noSubs = true;
        } else if (arg == "--embed-subs") {
            g_embedSubs = true;
        } else if (arg == "--lang" && i + 1 < argc) {
            g_subLang = argv[++i];
        }
    }

    std::cout << "Analyzing IMDB Media Signature..." << std::endl;
    std::string initial_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=tv";
    std::string raw_json = fetchURL(initial_url);

    if (raw_json.empty()) {
        std::cerr << "Failed to retrieve data from api endpoint." << std::endl;
        return 1;
    }

    json res = json::parse(stripToJSON(raw_json));
    std::string title = res["data"]["title"].get<std::string>();

    if (res["data"]["eps"].is_boolean() && res["data"]["eps"].get<bool>() == false) {
        std::string movie_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=movie";
        json movie_res = json::parse(stripToJSON(fetchURL(movie_url)));
        handleMovie(title, movie_res["data"]["stream_urls"]);
    } else {
        handleShow(imdb_id, title, res["data"]["eps"]);
    }

    return 0;
}
