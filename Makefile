CXX      = g++
CXXFLAGS = -std=c++17
LDFLAGS  = -lcurl
TARGET   = imdbdownloader
SRC      = downloader.cpp

# Detect platform and set include/lib paths accordingly
UNAME := $(shell uname)
ifeq ($(UNAME), Darwin)
    # Homebrew on Apple Silicon
    ifneq ($(wildcard /opt/homebrew/include),)
        CXXFLAGS += -I/opt/homebrew/include
        LDFLAGS  += -L/opt/homebrew/lib
    # Homebrew on Intel Mac
    else ifneq ($(wildcard /usr/local/include),)
        CXXFLAGS += -I/usr/local/include
        LDFLAGS  += -L/usr/local/lib
    endif
    INSTALL_DIR = /usr/local/bin
else
    # Linux: use pkg-config for curl if available
    ifneq ($(shell pkg-config --exists libcurl 2>/dev/null && echo yes),)
        CXXFLAGS += $(shell pkg-config --cflags libcurl)
        LDFLAGS  += $(shell pkg-config --libs libcurl)
    endif
    INSTALL_DIR = /usr/local/bin
endif

all: $(TARGET)

$(TARGET): $(SRC)
	$(CXX) $(CXXFLAGS) $(SRC) -o $(TARGET) $(LDFLAGS)

install: $(TARGET)
	install -m 755 $(TARGET) $(INSTALL_DIR)/$(TARGET)

clean:
	rm -f $(TARGET)
