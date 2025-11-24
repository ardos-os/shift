#include "../../include/tab_client.h"

#include <GLES2/gl2.h>
#include <poll.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

#include <png.h>

#ifndef DVD_ASSET_PATH
#define DVD_ASSET_PATH "dvd.png"
#endif

struct LogoState {
    float posX = 120.0f;
    float posY = 90.0f;
    float velX = 260.0f;
    float velY = 190.0f;
    size_t colorIndex = 0;

    void update(float dt, float fbWidth, float fbHeight, float logoW, float logoH);
    std::array<float, 3> tint() const;
};

constexpr std::array<std::array<float, 3>, 5> COLORS = {{
    {1.0f, 1.0f, 1.0f},
    {1.0f, 0.4f, 0.4f},
    {0.4f, 1.0f, 0.5f},
    {0.4f, 0.7f, 1.0f},
    {1.0f, 0.7f, 0.4f},
}};

struct GlResources {
    GLuint program = 0;
    GLuint vbo = 0;
    GLuint texture = 0;
    GLint attrPos = -1;
    GLint attrUv = -1;
    GLint uniResolution = -1;
    GLint uniPosition = -1;
    GLint uniSize = -1;
    GLint uniTint = -1;
    int texWidth = 0;
    int texHeight = 0;

    bool init(std::string& err);
std::pair<float, float> logoSize(int width, int height) const;
    void render(const TabFrameTarget& target, const LogoState& logo, float logoW, float logoH) const;
};

GLuint compileShader(GLenum type, const char* src, std::string& err);
GLuint linkProgram(GLuint vert, GLuint frag, std::string& err);
bool loadPng(std::vector<unsigned char>& pixels, int& width, int& height, std::string& err);
std::string takeError(TabClientHandle* client);
bool pumpEvents(TabClientHandle* client, bool blocking);
bool ensureMonitor(TabClientHandle* client, std::string& monitorId);
void refreshMonitorSelection(TabClientHandle* client, std::string& monitorId);

int main(int argc, char** argv) {
    const char* token = nullptr;
    if (argc > 1) {
        token = argv[1];
    } else {
        token = std::getenv("SHIFT_SESSION_TOKEN");
    }
    if (!token) {
        std::cerr << "Provide a session token via SHIFT_SESSION_TOKEN or argv[1]\n";
        return 1;
    }

    TabClientHandle* client = tab_client_connect_default(token);
    if (!client) {
        std::cerr << "tab_client_connect_default failed\n";
        return 1;
    }

    std::cout << "Connected to Shift\n";
    if (char* protocol = tab_client_get_protocol_name(client)) {
        std::cout << "  Protocol: " << protocol << "\n";
        tab_client_string_free(protocol);
    }
    if (char* server = tab_client_get_server_name(client)) {
        std::cout << "  Server: " << server << "\n";
        tab_client_string_free(server);
    }

    std::string monitorId;
    if (!ensureMonitor(client, monitorId)) {
        std::cerr << "No monitors available\n";
        tab_client_disconnect(client);
        return 1;
    }
    std::cout << "Using monitor " << monitorId << "\n";

    GlResources gl;
    std::string glErr;
    if (!gl.init(glErr)) {
        std::cerr << "Failed to init GL: " << glErr << "\n";
        tab_client_disconnect(client);
        return 1;
    }

    if (!tab_client_send_ready(client)) {
        std::cerr << "tab_client_send_ready failed: " << takeError(client) << "\n";
        tab_client_disconnect(client);
        return 1;
    }

    LogoState logo;
    auto last = std::chrono::steady_clock::now();

    while (true) {
        if (!ensureMonitor(client, monitorId)) {
            break;
        }

        TabFrameTarget target{};
        TabAcquireResult acquire = tab_client_acquire_frame(client, monitorId.c_str(), &target);
        if (acquire == TAB_ACQUIRE_ERROR) {
            std::cerr << "tab_client_acquire_frame failed: " << takeError(client) << "\n";
            break;
        }
        if (acquire == TAB_ACQUIRE_NO_BUFFERS) {
            if (!pumpEvents(client, true)) {
                break;
            }
            refreshMonitorSelection(client, monitorId);
            continue;
        }

        auto now = std::chrono::steady_clock::now();
        float dt = std::chrono::duration<float>(now - last).count();
        last = now;
        auto logoSize = gl.logoSize(target.width, target.height);
        logo.update(dt, static_cast<float>(target.width), static_cast<float>(target.height), logoSize.first, logoSize.second);
        gl.render(target, logo, logoSize.first, logoSize.second);

        if (!tab_client_swap_buffers(client, monitorId.c_str())) {
            std::cerr << "swap_buffers failed: " << takeError(client) << "\n";
            break;
        }

        if (!pumpEvents(client, false)) {
            break;
        }
        refreshMonitorSelection(client, monitorId);
    }

    tab_client_disconnect(client);
    return 0;
}

void LogoState::update(float dt, float fbWidth, float fbHeight, float logoW, float logoH) {
    const float maxX = std::max(fbWidth - logoW, 0.0f);
    const float maxY = std::max(fbHeight - logoH, 0.0f);
    posX = std::clamp(posX + velX * dt, 0.0f, maxX);
    posY = std::clamp(posY + velY * dt, 0.0f, maxY);
    bool bounced = false;
    if (posX <= 0.0f || posX >= maxX) {
        velX = -velX;
        bounced = true;
    }
    if (posY <= 0.0f || posY >= maxY) {
        velY = -velY;
        bounced = true;
    }
    if (bounced) {
        colorIndex = (colorIndex + 1) % COLORS.size();
    }
}

std::array<float, 3> LogoState::tint() const {
    return COLORS[colorIndex];
}

bool GlResources::init(std::string& err) {
    std::string vertSrc = R"(
attribute vec2 aPos;
attribute vec2 aUv;
varying vec2 vUv;
uniform vec2 uResolution;
uniform vec2 uPosition;
uniform vec2 uSize;
void main() {
    vec2 scaled = uPosition + aPos * uSize;
    vec2 clip = vec2(
        (scaled.x / uResolution.x) * 2.0 - 1.0,
        1.0 - (scaled.y / uResolution.y) * 2.0
    );
    gl_Position = vec4(clip, 0.0, 1.0);
    vUv = aUv;
}
)";
    std::string fragSrc = R"(
precision mediump float;
varying vec2 vUv;
uniform sampler2D uTexture;
uniform vec3 uTint;
void main() {
    vec4 tex = texture2D(uTexture, vUv);
    gl_FragColor = vec4((vec3(1.0) - tex.rgb) * uTint, tex.a);
}
)";

    GLuint vert = compileShader(GL_VERTEX_SHADER, vertSrc.c_str(), err);
    if (!vert) {
        return false;
    }
    GLuint frag = compileShader(GL_FRAGMENT_SHADER, fragSrc.c_str(), err);
    if (!frag) {
        glDeleteShader(vert);
        return false;
    }
    program = linkProgram(vert, frag, err);
    if (!program) {
        glDeleteShader(vert);
        glDeleteShader(frag);
        return false;
    }

    attrPos = glGetAttribLocation(program, "aPos");
    attrUv = glGetAttribLocation(program, "aUv");
    uniResolution = glGetUniformLocation(program, "uResolution");
    uniPosition = glGetUniformLocation(program, "uPosition");
    uniSize = glGetUniformLocation(program, "uSize");
    uniTint = glGetUniformLocation(program, "uTint");
    GLint uniTex = glGetUniformLocation(program, "uTexture");

    glGenBuffers(1, &vbo);
    const float vertices[] = {
        0.0f, 0.0f, 0.0f, 0.0f,
        1.0f, 0.0f, 1.0f, 0.0f,
        0.0f, 1.0f, 0.0f, 1.0f,
        1.0f, 1.0f, 1.0f, 1.0f,
    };
    glBindBuffer(GL_ARRAY_BUFFER, vbo);
    glBufferData(GL_ARRAY_BUFFER, sizeof(vertices), vertices, GL_STATIC_DRAW);
    const GLsizei stride = 4 * sizeof(float);
    glEnableVertexAttribArray(attrPos);
    glVertexAttribPointer(attrPos, 2, GL_FLOAT, GL_FALSE, stride, reinterpret_cast<void*>(0));
    glEnableVertexAttribArray(attrUv);
    glVertexAttribPointer(attrUv, 2, GL_FLOAT, GL_FALSE, stride, reinterpret_cast<void*>(2 * sizeof(float)));

    std::vector<unsigned char> pixels;
    if (!loadPng(pixels, texWidth, texHeight, err)) {
        return false;
    }
    glGenTextures(1, &texture);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, texture);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, texWidth, texHeight, 0, GL_RGBA, GL_UNSIGNED_BYTE, pixels.data());

    glUseProgram(program);
    glUniform1i(uniTex, 0);
    glEnable(GL_BLEND);
    glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    return true;
}

std::pair<float, float> GlResources::logoSize(int width, int height) const {
    const float w = static_cast<float>(width);
    const float h = static_cast<float>(height);
    const float aspect = static_cast<float>(texWidth) / static_cast<float>(texHeight);
    float desiredW = std::clamp(w * 0.25f, 80.0f, w * 0.9f);
    float desiredH = desiredW / aspect;
    if (desiredH > h * 0.5f) {
        desiredH = h * 0.5f;
        desiredW = desiredH * aspect;
    }
    return {desiredW, desiredH};
}

void GlResources::render(const TabFrameTarget& target, const LogoState& logo, float logoW, float logoH) const {
    glBindFramebuffer(GL_FRAMEBUFFER, target.framebuffer);
    glViewport(0, 0, target.width, target.height);
    glClearColor(0.02f, 0.02f, 0.04f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(program);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, texture);
    glUniform2f(uniResolution, static_cast<float>(target.width), static_cast<float>(target.height));
    glUniform2f(uniPosition, logo.posX, logo.posY);
    glUniform2f(uniSize, logoW, logoH);
    const auto tint = logo.tint();
    glUniform3f(uniTint, tint[0], tint[1], tint[2]);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
}

GLuint compileShader(GLenum type, const char* src, std::string& err) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &src, nullptr);
    glCompileShader(shader);
    GLint status = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &status);
    if (status == GL_FALSE) {
        GLint len = 0;
        glGetShaderiv(shader, GL_INFO_LOG_LENGTH, &len);
        std::string log(len, '\0');
        glGetShaderInfoLog(shader, len, nullptr, log.data());
        err = "Shader compilation failed: " + log;
        glDeleteShader(shader);
        return 0;
    }
    return shader;
}

GLuint linkProgram(GLuint vert, GLuint frag, std::string& err) {
    GLuint program = glCreateProgram();
    glAttachShader(program, vert);
    glAttachShader(program, frag);
    glLinkProgram(program);
    GLint status = 0;
    glGetProgramiv(program, GL_LINK_STATUS, &status);
    if (status == GL_FALSE) {
        GLint len = 0;
        glGetProgramiv(program, GL_INFO_LOG_LENGTH, &len);
        std::string log(len, '\0');
        glGetProgramInfoLog(program, len, nullptr, log.data());
        err = "Program link failed: " + log;
        glDeleteProgram(program);
        return 0;
    }
    glDetachShader(program, vert);
    glDetachShader(program, frag);
    glDeleteShader(vert);
    glDeleteShader(frag);
    return program;
}

bool loadPng(std::vector<unsigned char>& pixels, int& width, int& height, std::string& err) {
    FILE* file = std::fopen(DVD_ASSET_PATH, "rb");
    if (!file) {
        err = "Failed to open " DVD_ASSET_PATH;
        return false;
    }
    png_structp png = png_create_read_struct(PNG_LIBPNG_VER_STRING, nullptr, nullptr, nullptr);
    if (!png) {
        err = "png_create_read_struct failed";
        std::fclose(file);
        return false;
    }
    png_infop info = png_create_info_struct(png);
    if (!info) {
        err = "png_create_info_struct failed";
        png_destroy_read_struct(&png, nullptr, nullptr);
        std::fclose(file);
        return false;
    }
    if (setjmp(png_jmpbuf(png))) {
        err = "libpng read error";
        png_destroy_read_struct(&png, &info, nullptr);
        std::fclose(file);
        return false;
    }
    png_init_io(png, file);
    png_read_info(png, info);

    png_uint_32 w = png_get_image_width(png, info);
    png_uint_32 h = png_get_image_height(png, info);
    png_byte color_type = png_get_color_type(png, info);
    png_byte bit_depth = png_get_bit_depth(png, info);

    if (bit_depth == 16) png_set_strip_16(png);
    if (color_type == PNG_COLOR_TYPE_PALETTE) png_set_palette_to_rgb(png);
    if (color_type == PNG_COLOR_TYPE_GRAY && bit_depth < 8) png_set_expand_gray_1_2_4_to_8(png);
    if (png_get_valid(png, info, PNG_INFO_tRNS)) png_set_tRNS_to_alpha(png);
    if (color_type == PNG_COLOR_TYPE_RGB || color_type == PNG_COLOR_TYPE_GRAY || color_type == PNG_COLOR_TYPE_PALETTE)
        png_set_filler(png, 0xFF, PNG_FILLER_AFTER);
    if (color_type == PNG_COLOR_TYPE_GRAY || color_type == PNG_COLOR_TYPE_GRAY_ALPHA)
        png_set_gray_to_rgb(png);

    png_read_update_info(png, info);

    width = static_cast<int>(w);
    height = static_cast<int>(h);
    pixels.resize(static_cast<size_t>(width) * static_cast<size_t>(height) * 4);
    std::vector<png_bytep> rows(height);
    for (int y = 0; y < height; ++y) {
        rows[y] = pixels.data() + static_cast<size_t>(y) * static_cast<size_t>(width) * 4;
    }
    png_read_image(png, rows.data());

    png_destroy_read_struct(&png, &info, nullptr);
    std::fclose(file);
    return true;
}

std::string takeError(TabClientHandle* client) {
    std::string msg;
    if (char* err = tab_client_take_error(client)) {
        msg = err;
        tab_client_string_free(err);
    }
    return msg;
}

bool pumpEvents(TabClientHandle* client, bool blocking) {
    pollfd pfds[2];
    pfds[0].fd = tab_client_get_socket_fd(client);
    pfds[0].events = POLLIN;
    pfds[1].fd = tab_client_get_swap_fd(client);
    pfds[1].events = POLLIN;
    int timeout = blocking ? -1 : 0;
    int ready = poll(pfds, 2, timeout);
    if (ready < 0) {
        if (errno == EINTR) {
            return true;
        }
        perror("poll");
        return false;
    }
    if (ready == 0) {
        return true;
    }
    if (pfds[0].revents & POLLIN) {
        if (!tab_client_process_socket_events(client)) {
            std::cerr << "process_socket_events: " << takeError(client) << "\n";
            return false;
        }
    }
    if (pfds[1].revents & POLLIN) {
        if (!tab_client_process_swap_events(client)) {
            std::cerr << "process_swap_events: " << takeError(client) << "\n";
            return false;
        }
    }
    return true;
}

bool ensureMonitor(TabClientHandle* client, std::string& monitorId) {
    while (monitorId.empty()) {
        size_t count = tab_client_get_monitor_count(client);
        if (count == 0) {
            if (!pumpEvents(client, true)) {
                return false;
            }
            continue;
        }
        char* raw = tab_client_get_monitor_id(client, 0);
        if (!raw) {
            if (!pumpEvents(client, true)) {
                return false;
            }
            continue;
        }
        monitorId.assign(raw);
        tab_client_string_free(raw);
    }
    return true;
}

void refreshMonitorSelection(TabClientHandle* client, std::string& monitorId) {
    if (monitorId.empty()) {
        return;
    }
    size_t count = tab_client_get_monitor_count(client);
    bool found = false;
    for (size_t i = 0; i < count; ++i) {
        if (char* raw = tab_client_get_monitor_id(client, i)) {
            std::string id(raw);
            tab_client_string_free(raw);
            if (id == monitorId) {
                found = true;
                break;
            }
        }
    }
    if (!found) {
        monitorId.clear();
    }
}
