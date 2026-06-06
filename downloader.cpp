#include <iostream>
#include <string>
#include <vector>
#include <mutex>
#include <thread>
#include <atomic>
#include <sstream>
#include <iomanip>
#include <filesystem>
#include <curl/curl.h>
#include <nlohmann/json.hpp>
#include <sys/ioctl.h>
#include <unistd.h>
#include <regex>
#include <fstream>

using json = nlohmann::json;
namespace fs = std::filesystem;

// ── Global config ─────────────────────────────────────────────────────────────

struct Config {
    int threads = 3;
    int fragments = 8;
    std::string apiKey;
    bool embedSubs = false;
    std::string subLang = "English";
} g_config;

const std::string ANIAPI_BASE = "https://aniapi.kobosh.com";

// ── Utilities ─────────────────────────────────────────────────────────────────

std::string stripToJSON(const std::string& s) {
    size_t p = s.find_first_of("{[");
    if (p == std::string::npos) return s;
    return s.substr(p);
}

std::string trim(const std::string& s) {
    auto start = s.find_first_not_of(" \t\n\r\"'");
    if (start == std::string::npos) return "";
    auto end = s.find_last_not_of(" \t\n\r\"'");
    return s.substr(start, end - start + 1);
}

std::string sanitizeFilename(std::string name) {
    std::replace(name.begin(), name.end(), ' ', '_');
    std::regex re("[^a-zA-Z0-9_\\-]");
    return std::regex_replace(name, re, "");
}

size_t WriteCallback(void* contents, size_t size, size_t nmemb, void* userp) {
    ((std::string*)userp)->append((char*)contents, size * nmemb);
    return size * nmemb;
}

std::string fetchURL(const std::string& url, const std::string& apiKey) {
    CURL* curl;
    CURLcode res;
    std::string readBuffer;

    curl = curl_easy_init();
    if (curl) {
        struct curl_slist* headers = NULL;
        headers = curl_slist_append(headers, "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0");
        if (!apiKey.empty()) {
            headers = curl_slist_append(headers, ("x-api-key: " + apiKey).c_str());
        }
        curl_easy_setopt(curl, CURLOPT_URL, url.c_str());
        curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
        curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, WriteCallback);
        curl_easy_setopt(curl, CURLOPT_WRITEDATA, &readBuffer);
        curl_easy_setopt(curl, CURLOPT_FOLLOWLOCATION, 1L);
        curl_easy_setopt(curl, CURLOPT_TIMEOUT, 30L);

        res = curl_easy_perform(curl);
        curl_slist_free_all(headers);
        curl_easy_cleanup(curl);

        if (res != CURLE_OK) {
            throw std::runtime_error("CURL Error: " + std::string(curl_easy_strerror(res)));
        }
    }
    return readBuffer;
}

// ── Download Management & UI ──────────────────────────────────────────────────

struct Task {
    std::string season;
    int episode;
    std::string baseDir;
    std::string fileNameBase;
    std::string imdbId;
    bool downloaded = false;
    int claimedBy = -1; // -1 for unclaimed
    bool failed = false;
};

struct WorkerStatus {
    int id;
    std::string status = "Idle";
    double progress = 0;
    Task* currentTask = nullptr;
    std::string lastOutput;
};

class DownloadManager {
public:
    std::vector<Task> tasks;
    std::vector<WorkerStatus> workerStatus;
    std::mutex mtx;
    bool isBulk = false;
    int threadCount;

    DownloadManager(int threads) : threadCount(threads) {
        for (int i = 0; i < threads; ++i) {
            workerStatus.push_back({i + 1});
        }
    }

    void addTask(Task task) {
        tasks.push_back(task);
    }

    Task* claimTask(int workerId) {
        std::lock_guard<std::mutex> lock(mtx);
        for (auto& t : tasks) {
            if (t.claimedBy == -1 && !t.downloaded && !t.failed) {
                t.claimedBy = workerId;
                return &t;
            }
        }
        return nullptr;
    }

    void updateWorker(int workerId, std::string status, double progress, Task* task, std::string lastOut = "") {
        std::lock_guard<std::mutex> lock(mtx);
        for (auto& w : workerStatus) {
            if (w.id == workerId) {
                w.status = status;
                w.progress = progress;
                if (task) w.currentTask = task;
                if (!lastOut.empty()) w.lastOutput = lastOut;
                break;
            }
        }
        render();
    }

    void render() {
        if (!isBulk) return;

        int completed = 0;
        int failed = 0;
        for (const auto& t : tasks) {
            if (t.downloaded) completed++;
            if (t.failed) failed++;
        }
        int total = tasks.size();
        int processed = completed + failed;
        int percent = total > 0 ? (processed * 100 / total) : 0;

        struct winsize w;
        ioctl(STDOUT_FILENO, TIOCGWINSZ, &w);
        int terminalWidth = w.ws_col > 0 ? w.ws_col : 80;

        std::string failedText = failed > 0 ? ", " + std::to_string(failed) + " failed" : "";
        std::string statusText = " " + std::to_string(percent) + "% (" + std::to_string(processed) + "/" + std::to_string(total) + " episodes" + failedText + ")";
        std::string prefix = "Total Progress: ";

        int barWidth = std::max(10, terminalWidth - (int)prefix.length() - (int)statusText.length() - 2);
        int filledWidth = total > 0 ? (processed * barWidth / total) : 0;
        std::string bar = "[" + std::string(filledWidth, '#') + std::string(barWidth - filledWidth, '-') + "]";

        int linesToMove = (workerStatus.size() * 2) + 2;
        
        // Move cursor up
        std::cout << "\x1b[" << linesToMove << "A" << "\x1b[G";

        // Render Total Progress
        std::cout << "\x1b[K" << prefix << bar << statusText << "\n\x1b[K\n";

        for (const auto& ws : workerStatus) {
            std::string taskLabel = ws.currentTask ? "S" + ws.currentTask->season + "E" + std::to_string(ws.currentTask->episode) : "None";
            std::string statusLine = "Thread " + std::to_string(ws.id) + ": " + taskLabel;
            while(statusLine.length() < 18) statusLine += " ";
            statusLine += " | [" + ws.status + "]";
            
            if (statusLine.length() > (size_t)terminalWidth) statusLine = statusLine.substr(0, terminalWidth);
            std::cout << "\x1b[K" << statusLine << "\n";
            
            std::string out = ws.lastOutput;
            if (out.length() > (size_t)terminalWidth - 4) out = out.substr(0, terminalWidth - 4);
            std::cout << "\x1b[K  " << out << "\n";
        }
        std::cout.flush();
    }

    void startBulk() {
        isBulk = true;
        for (int i = 0; i < (threadCount * 2) + 2; ++i) std::cout << "\n";
        render();
    }
};

// ── AniAPI helpers ───────────────────────────────────────────────────────────

json fetchAniApi(const std::string& pathname) {
    std::string resp = fetchURL(ANIAPI_BASE + pathname, g_config.apiKey);
    try {
        return json::parse(stripToJSON(resp));
    } catch (const std::exception& e) {
        throw std::runtime_error("AniAPI response parse failed: " + std::string(e.what()) + "\nResponse: " + resp);
    }
}

struct Metadata {
    std::string title;
    std::string originalTitle;
    std::string type;
    std::vector<std::string> genres;
    int startYear = 0;
    json episodes;
    bool hasPrimaryStream = true;
};

Metadata fetchImdbMetadata(const std::string& imdbId) {
    try {
        json d = fetchAniApi("/info/" + imdbId);
        if (d.contains("error")) {
            throw std::runtime_error(d["error"].get<std::string>());
        }
        Metadata m;
        m.title = d.value("title", d.value("originalTitle", imdbId));
        m.originalTitle = d.value("originalTitle", d.value("title", imdbId));
        m.type = d.value("mediaType", d.value("type", "movie"));
        if (d.contains("genres")) m.genres = d["genres"].get<std::vector<std::string>>();
        m.startYear = d.value("year", 0);
        if (d.contains("episodes")) m.episodes = d["episodes"];
        m.hasPrimaryStream = d.value("hasPrimaryStream", true);
        return m;
    } catch (const std::exception& e) {
        std::cerr << "[Meta] AniAPI lookup failed: " << e.what() << std::endl;
        Metadata m;
        m.title = imdbId;
        m.originalTitle = imdbId;
        m.type = "movie";
        m.hasPrimaryStream = false;
        return m;
    }
}

bool isShowType(const std::string& type) {
    std::regex re("show|series|tv|mini|episode|special", std::regex_constants::icase);
    return std::regex_search(type, re);
}

// ── Subtitle management ──────────────────────────────────────────────────────

void handleSubtitles(const std::string& imdbId, const std::string& season, int episode, const std::string& videoPath) {
    if (!g_config.embedSubs) return;

    try {
        std::string path = (episode > 0)
            ? "/subtitles/show/" + imdbId + "/" + season + "/" + std::to_string(episode)
            : "/subtitles/movie/" + imdbId;

        std::cout << "[Subs] Fetching subtitles from " << path << "..." << std::endl;
        json subs = fetchAniApi(path);
        
        if (subs.empty()) {
            std::cout << "[Subs] No subtitles found." << std::endl;
            return;
        }

        // Try to find preferred language, fallback to first
        json selectedSub = subs[0];
        for (const auto& s : subs) {
            std::string lang = s.value("language", "");
            std::transform(lang.begin(), lang.end(), lang.begin(), ::tolower);
            std::string pref = g_config.subLang;
            std::transform(pref.begin(), pref.end(), pref.begin(), ::tolower);
            if (lang == pref) {
                selectedSub = s;
                break;
            }
        }

        std::string subUrl = selectedSub.value("url", "");
        if (subUrl.find("http") != 0) {
            subUrl = ANIAPI_BASE + subUrl;
        }

        std::cout << "[Subs] Downloading " << selectedSub.value("language", "Unknown") << " subtitle..." << std::endl;
        std::string subData = fetchURL(subUrl, g_config.apiKey);
        
        std::string srtPath = videoPath;
        size_t dot = srtPath.find_last_of(".");
        if (dot != std::string::npos) srtPath = srtPath.substr(0, dot) + ".srt";
        else srtPath += ".srt";

        std::ofstream out(srtPath);
        out << subData;
        out.close();

        std::cout << "[Subs] Embedding subtitle into " << videoPath << "..." << std::endl;
        std::string tempVideoPath = videoPath;
        dot = tempVideoPath.find_last_of(".");
        if (dot != std::string::npos) tempVideoPath = tempVideoPath.substr(0, dot) + ".temp.mp4";
        else tempVideoPath += ".temp.mp4";

        std::string lang = selectedSub.value("language", "eng");
        if (lang.length() > 3) lang = lang.substr(0, 3);
        std::transform(lang.begin(), lang.end(), lang.begin(), ::tolower);

        std::string cmd = "ffmpeg -y -i \"" + videoPath + "\" -i \"" + srtPath + "\" -c copy -c:s mov_text -metadata:s:s:0 language=" + lang + " \"" + tempVideoPath + "\" > /dev/null 2>&1";
        
        int res = std::system(cmd.c_str());
        if (res == 0) {
            fs::rename(tempVideoPath, videoPath);
            fs::remove(srtPath);
            std::cout << "[Subs] Subtitle embedded successfully." << std::endl;
        } else {
            std::cerr << "[Subs] ffmpeg failed with code " << res << std::endl;
            if (fs::exists(tempVideoPath)) fs::remove(tempVideoPath);
        }
    } catch (const std::exception& e) {
        std::cerr << "[Subs] Failed to embed subtitles: " << e.what() << std::endl;
    }
}

// ── Video downloader ──────────────────────────────────────────────────────────

void downloadStream(const std::string& m3u8Url, const std::string& outputPath, const json& extraHeaders, 
                    int fragments, int workerId = 0, DownloadManager* manager = nullptr) {
    
    std::string userAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0";
    if (extraHeaders.contains("User-Agent")) userAgent = extraHeaders["User-Agent"].get<std::string>();

    std::string cmd = "yt-dlp --user-agent \"" + userAgent + "\" --concurrent-fragments " + std::to_string(fragments) + " --extractor-args \"generic:impersonate\" --newline ";
    
    if (extraHeaders.contains("Referer")) {
        cmd += "--referer \"" + extraHeaders["Referer"].get<std::string>() + "\" ";
    }
    
    for (auto it = extraHeaders.begin(); it != extraHeaders.end(); ++it) {
        std::string key = it.key();
        if (key == "User-Agent" || key == "Referer") continue;
        cmd += "--add-header \"" + key + ":" + it.value().get<std::string>() + "\" ";
    }
    
    cmd += "\"" + m3u8Url + "\" -o \"" + outputPath + "\" 2>&1";

    FILE* pipe = popen(cmd.c_str(), "r");
    if (!pipe) throw std::runtime_error("Failed to run yt-dlp");

    char buffer[1024];
    std::regex progressRe("\\[download\\]\\s+([0-9]+\\.[0-9]+)%");
    
    while (fgets(buffer, sizeof(buffer), pipe) != NULL) {
        std::string line(buffer);
        if (line.back() == '\n') line.pop_back();
        if (line.empty()) continue;

        double progress = 0;
        std::smatch match;
        if (std::regex_search(line, match, progressRe)) {
            progress = std::stod(match[1].str());
        }

        if (manager) {
            manager->updateWorker(workerId, "Downloading", progress, nullptr, line);
        } else {
            std::cout << "\r\x1b[KStatus: Downloading... " << line << std::flush;
        }
    }

    int result = pclose(pipe);
    if (result != 0) throw std::runtime_error("yt-dlp failed with code " + std::to_string(result));
}

// ── Content handlers ──────────────────────────────────────────────────────────

void downloadWorker(int workerId, DownloadManager* manager) {
    while (true) {
        Task* task = manager->claimTask(workerId);
        if (!task) break;

        manager->updateWorker(workerId, "Downloading", 0, task);

        try {
            json epRes = fetchAniApi("/download/show/" + task->imdbId + "/" + task->season + "/" + std::to_string(task->episode));
            std::string m3u8 = epRes.value("streamUrl", "");
            json headers = epRes.value("headers", json::object());

            if (m3u8.empty()) throw std::runtime_error("No stream URL");

            fs::create_directories(task->baseDir);
            std::string outputPath = task->fileNameBase + ".mp4";

            downloadStream(m3u8, outputPath, headers, g_config.fragments, workerId, manager);

            handleSubtitles(task->imdbId, task->season, task->episode, outputPath);

            task->downloaded = true;
            manager->updateWorker(workerId, "Done", 100, task);
        } catch (const std::exception& e) {
            task->failed = true;
            std::string msg = e.what();
            if (msg.length() > 15) msg = msg.substr(0, 15);
            manager->updateWorker(workerId, "Error: " + msg, 0, task);
        }
    }
    manager->updateWorker(workerId, "Finished", 0, nullptr);
}

void handleMovie(const std::string& imdbId, const std::string& title) {
    json movieData;
    try {
        movieData = fetchAniApi("/download/movie/" + imdbId);
    } catch (...) {
        std::cerr << "No streams found for this movie." << std::endl;
        return;
    }

    std::string streamUrl = movieData.value("streamUrl", "");
    json headers = movieData.value("headers", json::object());
    if (streamUrl.empty()) {
        std::cerr << "No streams found for this movie." << std::endl;
        return;
    }

    std::string base = "./" + sanitizeFilename(title);
    std::cout << "\nFound Movie: " << title << std::endl;
    std::string outputPath = base + ".mp4";
    std::cout << "Downloading to " << outputPath << "..." << std::endl;
    
    downloadStream(streamUrl, outputPath, headers, g_config.fragments);
    handleSubtitles(imdbId, "", 0, outputPath);
    std::cout << "\nDownload complete." << std::endl;
}

void handleShow(const std::string& imdbId, const std::string& title, const json& epsData) {
    if (!epsData.is_null() && epsData.is_object() && !epsData.empty()) {
        std::cout << "\nFound TV Show: " << title << std::endl;
        std::cout << "Available Seasons:" << std::endl;
        
        std::vector<std::string> seasons;
        for (auto it = epsData.begin(); it != epsData.end(); ++it) {
            seasons.push_back(it.key());
            int count = it.value().is_array() ? it.value().size() : it.value().get<int>();
            std::cout << "  Season " << it.key() << " (" << count << " episodes)" << std::endl;
        }

        std::cout << "\nOptions:\n  1. Download one specific episode\n  2. Download ALL episodes" << std::endl;
        std::cout << "Choose an option (1-2): ";
        int mode;
        if (!(std::cin >> mode)) return;
        std::string cleanTitle = sanitizeFilename(title);

        if (mode == 1) {
            std::string chosenSeason;
            int chosenEp;
            std::cout << "Enter Season Number: ";
            std::cin >> chosenSeason;
            std::cout << "Enter Episode Number: ";
            std::cin >> chosenEp;

            try {
                json epRes = fetchAniApi("/download/show/" + imdbId + "/" + chosenSeason + "/" + std::to_string(chosenEp));
                std::string streamUrl = epRes.value("streamUrl", "");
                json headers = epRes.value("headers", json::object());
                if (!streamUrl.empty()) {
                    std::string base = "./" + cleanTitle + "-S" + chosenSeason + "-E" + std::to_string(chosenEp);
                    std::string outputPath = base + ".mp4";
                    std::cout << "\nDownloading S" << chosenSeason << "E" << chosenEp << "..." << std::endl;
                    downloadStream(streamUrl, outputPath, headers, g_config.fragments);
                    handleSubtitles(imdbId, chosenSeason, chosenEp, outputPath);
                    std::cout << "\nDownload complete." << std::endl;
                } else {
                    std::cerr << "No stream found via primary source." << std::endl;
                }
            } catch (...) {
                std::cerr << "Primary source failed for that episode." << std::endl;
            }
        } else if (mode == 2) {
            DownloadManager manager(g_config.threads);
            for (const auto& s : seasons) {
                int epCount = epsData[s].is_array() ? epsData[s].size() : epsData[s].get<int>();
                for (int ep = 1; ep <= epCount; ++ep) {
                    Task t;
                    t.season = s;
                    t.episode = ep;
                    t.baseDir = "./" + cleanTitle + "/Season_" + s;
                    t.fileNameBase = t.baseDir + "/" + cleanTitle + "-S" + s + "-E" + std::to_string(ep);
                    t.imdbId = imdbId;
                    manager.addTask(t);
                }
            }

            std::cout << "\nStarting bulk download (" << manager.tasks.size() << " episodes) with " << g_config.threads << " threads..." << std::endl;
            manager.startBulk();

            std::vector<std::thread> workers;
            for (int i = 0; i < g_config.threads; ++i) {
                workers.emplace_back(downloadWorker, i + 1, &manager);
            }
            for (auto& w : workers) w.join();
            std::cout << "\nAll downloads completed." << std::endl;
        } else {
            std::cerr << "Invalid option." << std::endl;
        }
        return;
    }

    std::cout << "\nFound TV Show: " << title << std::endl;
    std::cout << "[Info] AniAPI did not return episode metadata. Downloading a single episode only." << std::endl;
    std::string cleanTitle = sanitizeFilename(title);
    std::string chosenSeason;
    int chosenEp;
    std::cout << "Enter Season Number: ";
    std::cin >> chosenSeason;
    std::cout << "Enter Episode Number: ";
    std::cin >> chosenEp;

    try {
        json epRes = fetchAniApi("/download/show/" + imdbId + "/" + chosenSeason + "/" + std::to_string(chosenEp));
        std::string streamUrl = epRes.value("streamUrl", "");
        json headers = epRes.value("headers", json::object());
        if (streamUrl.empty()) {
            std::cerr << "No stream found for that episode." << std::endl;
            return;
        }
        std::string base = "./" + cleanTitle + "-S" + chosenSeason + "-E" + std::to_string(chosenEp);
        std::string outputPath = base + ".mp4";
        std::cout << "\nDownloading S" << chosenSeason << "E" << chosenEp << "..." << std::endl;
        downloadStream(streamUrl, outputPath, headers, g_config.fragments);
        handleSubtitles(imdbId, chosenSeason, chosenEp, outputPath);
        std::cout << "\nDownload complete." << std::endl;
    } catch (const std::exception& e) {
        std::cerr << "AniAPI episode download failed: " << e.what() << std::endl;
    }
}

// ── Dependency check ──────────────────────────────────────────────────────────

bool checkDependencies() {
    int res = std::system("command -v yt-dlp > /dev/null 2>&1");
    if (res != 0) {
        std::cerr << "Missing required dependencies: yt-dlp\n";
        return false;
    }
    res = std::system("command -v ffmpeg > /dev/null 2>&1");
    if (res != 0) {
        std::cerr << "Missing required dependencies: ffmpeg\n";
        return false;
    }
    return true;
}

// ── Main ──────────────────────────────────────────────────────────────────────

void printHelp() {
    std::cout << "Usage: imdbdownloader <imdb_id> [options]\n"
              << "Options:\n"
              << "  --key <apikey>                   AniAPI key (falls back to ANIAPI_TOKEN env var)\n"
              << "  -t, --threads <number>           Number of concurrent downloads (shows only) [default: 3]\n"
              << "  --concurrent-fragments <number>  Number of concurrent fragments per download [default: 8]\n"
              << "  --embed-subs                     Automatically download and embed subtitles\n"
              << "  --sub-lang <lang>                Preferred subtitle language [default: English]\n\n"
              << "Examples:\n"
              << "  $ imdbdownloader tt0480489 --embed-subs\n"
              << "  $ imdbdownloader tt0480489 --key YOUR_API_KEY --embed-subs --sub-lang Hungarian\n";
}

int main(int argc, char* argv[]) {
    if (argc < 2) {
        printHelp();
        return 1;
    }

    std::string imdbId = argv[1];
    if (imdbId == "--help" || imdbId == "-h") {
        printHelp();
        return 0;
    }

    const char* envToken = std::getenv("ANIAPI_TOKEN");
    if (envToken) g_config.apiKey = trim(envToken);

    for (int i = 2; i < argc; ++i) {
        std::string arg = argv[i];
        if (arg == "--key" && i + 1 < argc) {
            g_config.apiKey = trim(argv[++i]);
        } else if ((arg == "-t" || arg == "--threads") && i + 1 < argc) {
            g_config.threads = std::stoi(argv[++i]);
        } else if (arg == "--concurrent-fragments" && i + 1 < argc) {
            g_config.fragments = std::stoi(argv[++i]);
        } else if (arg == "--embed-subs") {
            g_config.embedSubs = true;
        } else if (arg == "--sub-lang" && i + 1 < argc) {
            g_config.subLang = argv[++i];
        }
    }

    if (g_config.apiKey.empty()) {
        std::cerr << "Error: API key required. Contact @kobosh_com on telegram/@kobosh.com on discord for a api key" << std::endl;
        return 1;
    }

    if (!checkDependencies()) return 1;

    curl_global_init(CURL_GLOBAL_DEFAULT);

    std::cout << "Analyzing IMDB Media Signature..." << std::endl;

    try {
        Metadata meta = fetchImdbMetadata(imdbId);
        std::cout << "\nTitle: " << meta.title << " (" << meta.type << ")" << std::endl;

        if (!isShowType(meta.type)) {
            handleMovie(imdbId, meta.title);
        } else {
            handleShow(imdbId, meta.title, meta.episodes);
        }
    } catch (const std::exception& e) {
        std::cerr << "Error: " << e.what() << std::endl;
    }

    curl_global_cleanup();
    return 0;
}
