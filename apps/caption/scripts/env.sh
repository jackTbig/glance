# 编译环境（Git Bash 用）：source scripts/env.sh
# 与用户级环境变量一致；当前会话的 shell 继承不到新写的变量，故显式导出
export VULKAN_SDK="D:/VulkanSDK"
export LIBCLANG_PATH="D:/Tools/LLVM/bin"
export PATH="/d/Tools/CMake/bin:/d/Tools/LLVM/bin:/d/VulkanSDK/Bin:$PATH"
# whisper.cpp 的 vulkan-shaders-gen 是嵌套 CMake 工程，VS 生成器下路径超 260 字符会让 MSBuild 崩溃：
# 一律用 Ninja（pip install ninja 提供），并把 target 放到短路径
export CMAKE_GENERATOR="Ninja"
export CARGO_TARGET_DIR="D:/DevCache/lc-target"
