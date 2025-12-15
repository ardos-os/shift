#include "../include/tab_client.h"
/* NOLINTBEGIN */

#include <GLES2/gl2.h>
#include <poll.h>
#include <tab_client.h>
#include <unistd.h>

#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <optional>
#include <string>
#include <vector>

/* ============================================================================
 * stb_image
 * ============================================================================
 */
#define STB_IMAGE_IMPLEMENTATION
#include <stb/stb_image.h>


struct TabString {
    char* ptr = nullptr;
    explicit TabString(char* p = nullptr) : ptr(p) {}
    ~TabString() { if (ptr) tab_client_string_free(ptr); }
    std::string str() const { return ptr ? std::string(ptr) : std::string(); }
};

static void die(const char* msg) {
    std::cerr << msg << "\n";
    std::exit(1);
}

struct Spinner {
    float phase = 0.0f;

    void update(float dt) {
        phase += dt * 1.5f;
    }

    float scale() const {
        return std::sin(phase);
    }
};


GLuint compile_shader(GLenum type, const char* src) {
    GLuint shader = glCreateShader(type);
    glShaderSource(shader, 1, &src, nullptr);
    glCompileShader(shader);

    GLint ok = 0;
    glGetShaderiv(shader, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        GLint len = 0;
        glGetShaderiv(shader, GL_INFO_LOG_LENGTH, &len);
        std::string log(len, '\0');
        glGetShaderInfoLog(shader, len, nullptr, log.data());
        die(log.c_str());
    }
    return shader;
}

GLuint link_program(GLuint vs, GLuint fs) {
    GLuint prog = glCreateProgram();
    glAttachShader(prog, vs);
    glAttachShader(prog, fs);
    glLinkProgram(prog);

    GLint ok = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &ok);
    if (!ok) {
        GLint len = 0;
        glGetProgramiv(prog, GL_INFO_LOG_LENGTH, &len);
        std::string log(len, '\0');
        glGetProgramInfoLog(prog, len, nullptr, log.data());
        die(log.c_str());
    }

    glDeleteShader(vs);
    glDeleteShader(fs);
    return prog;
}

struct Renderer {
    GLuint program = 0;
    GLuint vbo = 0;
    GLuint texture = 0;

    GLint u_resolution = -1;
    GLint u_center = -1;
    GLint u_size = -1;
    GLint u_scale = -1;

    int tex_w = 0;
    int tex_h = 0;

    void init(const char* png_path) {
        const char* vert_src = R"(
attribute vec2 aPos;
varying vec2 vUv;
uniform vec2 uResolution;
uniform vec2 uCenter;
uniform vec2 uSize;
uniform float uScaleX;

void main() {
    vec2 halfSize = uSize * 0.5;
    vec2 scaled = vec2(aPos.x * halfSize.x * uScaleX,
                       aPos.y * halfSize.y);
    vec2 pixel = uCenter + scaled;

    vec2 clip = vec2(
        (pixel.x / uResolution.x) * 2.0 - 1.0,
        1.0 - (pixel.y / uResolution.y) * 2.0
    );

    gl_Position = vec4(clip, 0.0, 1.0);
    vUv = (aPos + 1.0) * 0.5;
}
)";

        const char* frag_src = R"(
precision mediump float;
varying vec2 vUv;
uniform sampler2D uTexture;

void main() {
    gl_FragColor = texture2D(uTexture, vUv);
}
)";

        GLuint vs = compile_shader(GL_VERTEX_SHADER, vert_src);
        GLuint fs = compile_shader(GL_FRAGMENT_SHADER, frag_src);
        program = link_program(vs, fs);

        u_resolution = glGetUniformLocation(program, "uResolution");
        u_center     = glGetUniformLocation(program, "uCenter");
        u_size       = glGetUniformLocation(program, "uSize");
        u_scale      = glGetUniformLocation(program, "uScaleX");

        // Quad
        const float verts[] = {
            -1, -1,
             1, -1,
            -1,  1,
             1,  1,
        };

        glGenBuffers(1, &vbo);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        glBufferData(GL_ARRAY_BUFFER, sizeof(verts), verts, GL_STATIC_DRAW);

        // Texture
        int n = 0;
        unsigned char* img = stbi_load(png_path, &tex_w, &tex_h, &n, 4);
        if (!img) die("Failed to load PNG");

        glGenTextures(1, &texture);
        glBindTexture(GL_TEXTURE_2D, texture);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
        glTexImage2D(
            GL_TEXTURE_2D, 0, GL_RGBA,
            tex_w, tex_h, 0,
            GL_RGBA, GL_UNSIGNED_BYTE, img
        );

        stbi_image_free(img);

        glEnable(GL_BLEND);
        glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
    }

    void draw(const TabFrameTarget& target, float scale) {
        glBindFramebuffer(GL_FRAMEBUFFER, target.framebuffer);
        glViewport(0, 0, target.width, target.height);

        glClearColor(1.0f, 0.75f, 0.8f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);

        glUseProgram(program);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);

        GLint aPos = glGetAttribLocation(program, "aPos");
        glEnableVertexAttribArray(aPos);
        glVertexAttribPointer(aPos, 2, GL_FLOAT, GL_FALSE, 0, nullptr);

        float aspect = float(tex_w) / float(tex_h);
        float w = target.width * 0.5f;
        float h = w / aspect;
        if (h > target.height * 0.6f) {
            h = target.height * 0.6f;
            w = h * aspect;
        }

        glUniform2f(u_resolution, target.width, target.height);
        glUniform2f(u_center, target.width * 0.5f, target.height * 0.5f);
        glUniform2f(u_size, w, h);
        glUniform1f(u_scale, scale);

        glActiveTexture(GL_TEXTURE0);
        glBindTexture(GL_TEXTURE_2D, texture);

        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    }
};

/* ============================================================================
 * Event handling
 * ============================================================================
 */

void handle_event(const TabEvent& ev,
                  std::optional<std::string>& monitor_id) {
    switch (ev.event_type) {
        case TAB_EVENT_MONITOR_ADDED:
            if (!monitor_id)
                monitor_id = ev.data.monitor_added.id;
			std::cout << "[CPP PENGER] Monitor added: "
					  << *monitor_id << "\n";
            break;

        case TAB_EVENT_MONITOR_REMOVED:
            if (monitor_id &&
                monitor_id == ev.data.monitor_removed)
                monitor_id.reset();
            break;

        default:
            break;
    }
}

/* ============================================================================
 * Main
 * ============================================================================
 */

int main(int argc, char** argv) {
    const char* token =
        argc > 1 ? argv[1] : std::getenv("SHIFT_SESSION_TOKEN");
    if (!token) die("Missing session token");

    TabClientHandle* client = tab_client_connect_default(token);
    if (!client) die("Failed to connect");

    TabString server(tab_client_get_server_name(client));
    TabString proto(tab_client_get_protocol_name(client));
    std::cout << "[CPP PENGER] Connected to " << server.str()
              << " via " << proto.str() << "\n";
	std::optional<std::string> monitor_id;
    // Wait for monitor
    while (tab_client_get_monitor_count(client) == 0) {
        tab_client_poll_events(client);
    }
	monitor_id = tab_client_get_monitor_id(client, 0);
    tab_client_send_ready(client);

    Renderer renderer;
    renderer.init("tab-client/examples/penger.png");

    Spinner spinner;
    auto last = std::chrono::steady_clock::now();

    while (true) {
        TabFrameTarget target{};
        TabAcquireResult res =
            tab_client_acquire_frame(
                client,
                monitor_id->c_str(),
                &target
            );

        if (res == TAB_ACQUIRE_OK) {
            auto now = std::chrono::steady_clock::now();
            float dt =
                std::chrono::duration<float>(now - last).count();
            last = now;
            
            spinner.update(std::max(dt, 1.0f / 240.0f));
            renderer.draw(target, spinner.scale());
            
            tab_client_swap_buffers(client, monitor_id->c_str());
        }

        tab_client_poll_events(client);
        TabEvent ev;
        while (tab_client_next_event(client, &ev)) {
            handle_event(ev, monitor_id);
            tab_client_free_event_strings(&ev);
        }
    }
}
/* NOLINTEND */
