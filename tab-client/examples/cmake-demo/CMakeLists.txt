cmake_minimum_required(VERSION 3.16)
project(tab_client_cmake_demo CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Point to the in-repo tab-client.
set(TAB_CLIENT_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
list(APPEND CMAKE_MODULE_PATH "${TAB_CLIENT_ROOT}/cmake")

# Choose build dir to match the current build type (debug vs release).
if (CMAKE_BUILD_TYPE MATCHES "^[Rr]elease" OR CMAKE_BUILD_TYPE MATCHES "RelWithDebInfo" OR CMAKE_BUILD_TYPE MATCHES "MinSizeRel")
    set(TAB_CLIENT_BUILD "${TAB_CLIENT_ROOT}/../target/release")
else ()
    set(TAB_CLIENT_BUILD "${TAB_CLIENT_ROOT}/../target/debug")
endif ()

# Load helper + find_package definitions.
include(FindTabClient)

# Build the Rust library (profile inferred from CMAKE_BUILD_TYPE).
tab_client_add_build_target()

# Locate the built artifacts and header.
find_package(TabClient REQUIRED)

add_executable(tab_client_demo main.cpp)
add_dependencies(tab_client_demo tab_client)
target_link_libraries(tab_client_demo PRIVATE TabClient::TabClient GL)
target_include_directories(tab_client_demo PRIVATE ${TabClient_INCLUDE_DIR})
