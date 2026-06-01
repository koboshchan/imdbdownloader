CXX      = g++
CXXFLAGS = -std=c++17 -I/opt/homebrew/include
LDFLAGS  = -L/opt/homebrew/lib -lcurl
TARGET   = imdbdownloader
SRC      = downloader.cpp

all: $(TARGET)

$(TARGET): $(SRC)
	$(CXX) $(CXXFLAGS) $(SRC) -o $(TARGET) $(LDFLAGS)

install: $(TARGET)
	install -m 755 $(TARGET) $(HOME)/bin/$(TARGET)

clean:
	rm -f $(TARGET)
