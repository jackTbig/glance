# 编译环境（Git Bash 用）：source scripts/env.sh
# 与用户级环境变量一致；当前会话的 shell 继承不到新写的变量，故显式导出
export VULKAN_SDK="D:/VulkanSDK"
export LIBCLANG_PATH="D:/Tools/LLVM/bin"
export PATH="/d/Tools/CMake/bin:/d/Tools/LLVM/bin:/d/VulkanSDK/Bin:$PATH"
