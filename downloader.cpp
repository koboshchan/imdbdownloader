#include <iostream>
#include <string>
#include <vector>
#include <cstdlib>
#include <sstream>
#include <memory>
#include <algorithm>
#include <curl/curl.h>
#include <nlohmann/json.hpp>

using json = nlohmann::json;

size_t WriteCallback(void* contents, size_t size, size_t nmemb, void* userp) {
    ((std::string*)userp)->append((char*)contents, size * nmemb);
    return size * nmemb;
}

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

        res = curl_easy_perform(curl);
        curl_slist_free_all(headers);
        curl_easy_cleanup(curl);

        if (res != CURLE_OK) {
            std::cerr << "CURL Error: " << curl_easy_strerror(res) << std::endl;
        }
    }
    return readBuffer;
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
    std::string filename = "./" + sanitizeFilename(title) + ".mp4";
    std::cout << "\nFound Movie: " << title << std::endl;
    std::cout << "Downloading to " << filename << "..." << std::endl;
    downloadStream(stream_urls[0].get<std::string>(), filename);
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
        json ep_res = json::parse(fetchURL(ep_url));
        auto urls = ep_res["data"]["stream_urls"];

        if (!urls.empty()) {
            std::string out = "./" + cleanTitle + "-S" + chosen_season + "-E" + std::to_string(chosen_ep) + ".mp4";
            downloadStream(urls[0].get<std::string>(), out);
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
                    json ep_res = json::parse(fetchURL(ep_url));
                    auto urls = ep_res["data"]["stream_urls"];
                    if (!urls.empty()) {
                        std::string dir_cmd = "mkdir -p \"./" + cleanTitle + "/Season_" + season_num + "\"";
                        std::system(dir_cmd.c_str());

                        std::string out = "./" + cleanTitle + "/Season_" + season_num + "/" + cleanTitle + "-S" + season_num + "-E" + std::to_string(ep) + ".mp4";
                        downloadStream(urls[0].get<std::string>(), out);
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
        std::cerr << "Usage: " << argv[0] << " <IMDB_ID> (e.g., tt0480489)" << std::endl;
        return 1;
    }

    std::string imdb_id = argv[1];

    std::cout << "Analyzing IMDB Media Signature..." << std::endl;
    std::string initial_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=tv";
    std::string raw_json = fetchURL(initial_url);

    if (raw_json.empty()) {
        std::cerr << "Failed to retrieve data from api endpoint." << std::endl;
        return 1;
    }

    json res = json::parse(raw_json);
    std::string title = res["data"]["title"].get<std::string>();

    if (res["data"]["eps"].is_boolean() && res["data"]["eps"].get<bool>() == false) {
        std::string movie_url = "https://streamdata.vaplayer.ru/api.php?imdb=" + imdb_id + "&type=movie";
        json movie_res = json::parse(fetchURL(movie_url));
        handleMovie(title, movie_res["data"]["stream_urls"]);
    } else {
        handleShow(imdb_id, title, res["data"]["eps"]);
    }

    return 0;
}
