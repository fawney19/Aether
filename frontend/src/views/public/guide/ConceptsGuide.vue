<script setup lang="ts">
import { BookOpen } from 'lucide-vue-next'
</script>

<template>
  <div class="space-y-12">
    <!-- Hero 区域 -->
    <div class="space-y-4">
      <div class="inline-flex items-center gap-1.5 rounded-full bg-[#cc785c]/10 dark:bg-[#cc785c]/20 border border-[#cc785c]/20 dark:border-[#cc785c]/40 px-3 py-1 text-xs font-medium text-[#cc785c] dark:text-[#d4a27f]">
        <BookOpen class="h-3 w-3" />
        文档核心
      </div>
      <h1 class="text-3xl font-bold text-[#262624] dark:text-[#f1ead8]">
        相关概念
      </h1>
      <p class="text-base text-[#666663] dark:text-[#a3a094] max-w-2xl">
        深入理解 Aether 运行时的核心模块和层级结构。
      </p>
    </div>

    <!-- 1. 创建统一模型 -->
    <section
      id="create-model"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>1. 创建统一模型</h2>
      <div class="space-y-4 mt-4">
        <div>
          <h4 class="font-medium text-[#262624] dark:text-[#f1ead8]">
            1. 左侧模型选择、搜索区域
          </h4>
          <ul class="list-disc pl-5 mt-2 space-y-1 text-sm text-[#666663] dark:text-[#a3a094]">
            <li>模型来源：opencode 维护的 https://models.dev</li>
            <li>仅显示几个官方模型提供商，可以通过搜索获取更多模型。</li>
          </ul>
        </div>
        <div>
          <h4 class="font-medium text-[#262624] dark:text-[#f1ead8]">
            2. 右侧参数配置区域
          </h4>
          <ul class="list-disc pl-5 mt-2 space-y-1 text-sm text-[#666663] dark:text-[#a3a094]">
            <li>手动添加模型 或 选择模型后自动填写。</li>
            <li>模型偏好：标定这个模型是否支持这些能力，一般情况下可不选。</li>
            <li>
              价格配置：
              <ul class="list-[circle] pl-5 mt-1 space-y-1">
                <li>计费方式：Token价格 + 按次价格 + 视频计费 = 最终计费</li>
                <li>价格阶梯：以总Token数落在哪个区间为准，并非按阶梯溢出补全式计费</li>
              </ul>
            </li>
          </ul>
        </div>
        <img
          loading="lazy"
          src="/guide/concepts-create-model.webp"
          alt="创建统一模型"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm mt-4 w-full"
        >
      </div>
    </section>

    <!-- 2. 添加提供商 -->
    <section
      id="add-provider"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>2. 添加提供商</h2>
      <div class="space-y-4 mt-4 text-[#666663] dark:text-[#a3a094] text-sm">
        <ul class="list-decimal pl-5 space-y-2">
          <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">提供商类型：</strong>自定义或反代；一般自定义即可，反代请进入反代章节。</li>
          <li>
            <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">计费类型：</strong>
            <ul class="list-disc pl-5 mt-1 space-y-1">
              <li>按量付费：持续使用</li>
              <li>月卡额度：按周期(天)限额</li>
              <li>免费套餐：不计入成本即倍率为0</li>
            </ul>
          </li>
          <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">最大重试次数：</strong>在缓存亲和调度模式下，首次请求失败后的重试次数。</li>
          <li>
            <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">超时时间：</strong>
            <ul class="list-disc pl-5 mt-1 space-y-1">
              <li>流式首字超时时间：流式请求收到首字前的超时时间</li>
              <li>非流请求超时时间：非流请求的总超时时间</li>
            </ul>
          </li>
          <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">保持优先级：</strong>通过格式转换的请求，是否保持当前优先级。</li>
        </ul>
        <img
          loading="lazy"
          src="/guide/concepts-add-provider.webp"
          alt="添加提供商"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm mt-4 w-full"
        >
      </div>
    </section>

    <!-- 3. 添加端点 -->
    <section
      id="add-endpoint"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>3. 添加端点</h2>
      <p class="text-sm text-[#666663] dark:text-[#a3a094] mb-4">
        添加端点是添加上游支持的端点，并非你需要使用的端点。比如 Anyrouter 只支持 Claude Code 接入，那么就应该添加 Claude Messages 端点，并在 Key 中选择对应认证方式。
      </p>
      <ul class="list-decimal pl-5 space-y-2 text-[#666663] dark:text-[#a3a094] text-sm">
        <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">选择格式：</strong>选择上游支持的端点格式。</li>
        <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">自定义 Base URL、Path：</strong>根据上游提供的信息填写。为空则使用提示的默认值。</li>
      </ul>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mt-6">
        <img
          loading="lazy"
          src="/guide/concepts-add-endpoint-1.webp"
          alt="添加端点选择格式"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
        <img
          loading="lazy"
          src="/guide/concepts-add-endpoint-2.webp"
          alt="端点自定义"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
      </div>
    </section>

    <!-- 4. 添加密钥 -->
    <section
      id="add-key"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>4. 添加密钥</h2>
      <ul class="list-decimal pl-5 space-y-3 mt-4 text-[#666663] dark:text-[#a3a094] text-sm">
        <li>
          <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">认证类型：</strong>
          <ul class="list-disc pl-5 mt-1">
            <li>API Key: 传统密钥</li>
            <li>Vertex AI: 用于 Google Cloud</li>
          </ul>
        </li>
        <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">支持的API格式：</strong>勾选Key可以使用的API格式，同时可以设置key访问该端点时的倍率。</li>
        <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">优先级：</strong>在提供商优先的调度模式下，不同优先级的Key按顺序故障转移，同优先级的Key进行负载均衡。</li>
        <li>
          <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">RPM限制：</strong>Key的使用速率限制，会预留10%给已有用户进行提高缓存命中率。
          <ul class="list-disc pl-5 mt-1">
            <li>填写固定数值。</li>
            <li>不填为空，采用一定策略自适应学习。</li>
          </ul>
        </li>
        <li>
          <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">缓存TTL：</strong>
          <ul class="list-disc pl-5 mt-1">
            <li>0: 不使用缓存优先</li>
            <li>N: 同一个用户Key在N分钟内，优先使用之前使用的提供商、Key响应请求。</li>
          </ul>
        </li>
        <li>
          <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">熔断探测：</strong><br>
          当同一个提供商Key字连续若干次请求失败后，会进入熔断状态，之后每间N分钟进行探测请求，若请求成功解除熔断后续正常请求，否则按以指数级增长探测时间以待下次探测，最大探测间隔不会增长超过32分钟。
        </li>
        <li><strong class="text-[#262624] dark:text-[#f1ead8] font-medium">能力标签：</strong>定义该Key可以使用的能力。</li>
        <li>
          <strong class="text-[#262624] dark:text-[#f1ead8] font-medium">自动获取上游模型：</strong><br>
          在上游获取模型端点支持的情况下，从接口自动获取可以用模型列表。且按一定时间自动刷新，不开启则默认任意模型可用，或在后续模型权限中手动添加。
        </li>
      </ul>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mt-6">
        <img
          loading="lazy"
          src="/guide/concepts-add-key-1.webp"
          alt="添加密钥认证"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
        <img
          loading="lazy"
          src="/guide/concepts-add-key-2.webp"
          alt="密钥倍率与优先级"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
      </div>
    </section>

    <!-- 5. 模型权限 -->
    <section
      id="model-permission"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>5. 模型权限</h2>
      <ul class="list-decimal pl-5 space-y-2 mt-4 text-[#666663] dark:text-[#a3a094] text-sm">
        <li>在编辑密钥中开启自动获取/刷新上游全部模型。</li>
        <li>手动限制/创建该Key的可用模型。</li>
        <li>不开启自动获取且勾选任意模型，即模型权限为空，则认为可以使用提供商的全部关联模型。</li>
        <li>只有在模型权限不为空的情况下，才可以配合正则映射模型。</li>
      </ul>
      <div class="grid grid-cols-1 gap-4 mt-6">
        <img
          loading="lazy"
          src="/guide/concepts-model-perms-1.webp"
          alt="模型权限 1"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
          <img
            loading="lazy"
            src="/guide/concepts-model-perms-2.webp"
            alt="模型权限 2"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
          <img
            loading="lazy"
            src="/guide/concepts-model-perms-3.webp"
            alt="模型权限 3"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
        </div>
      </div>
    </section>

    <!-- 6. 关联模型 -->
    <section
      id="link-model"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>6. 关联模型</h2>
      <p class="text-sm text-[#666663] dark:text-[#a3a094] mb-4">
        从全局模型关联提供商模型，即设置该提供商可以访问的模型，同时可以在这里设置提供商自己的模型价格。默认继承全局模型的价格。
      </p>
      <div class="grid grid-cols-1 gap-4 mt-6">
        <img
          loading="lazy"
          src="/guide/concepts-link-model-1.webp"
          alt="关联模型 1"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
          <img
            loading="lazy"
            src="/guide/concepts-link-model-2.webp"
            alt="关联模型 2"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
          <img
            loading="lazy"
            src="/guide/concepts-link-model-3.webp"
            alt="关联模型 3"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
        </div>
      </div>
    </section>

    <!-- 7. 模型映射 -->
    <section
      id="model-mapping"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>7. 模型映射</h2>
      <p class="text-sm text-[#666663] dark:text-[#a3a094] mb-4">
        如果该提供商的请求名称并非标准名称，即可通过映射改变在实际请求提供时的模型 id。
      </p>
      <div class="bg-[#f5f5f0]/80 dark:bg-[rgba(227,224,211,0.05)] p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] text-sm mb-6">
        <p class="mb-2">
          <span class="text-[#cc785c] font-medium mr-2">名称修正示例:</span>官方标准名称为 <code class="bg-black/5 dark:bg-white/10 px-1 py-0.5 rounded text-xs">claude-opus-4-6</code> ，实际提供商叫 <code class="bg-black/5 dark:bg-white/10 px-1 py-0.5 rounded text-xs">claude-opus-4-6-last</code>。
        </p>
        <p><span class="text-[#cc785c] font-medium mr-2">降/升级请求示例:</span>官方标准名称为 <code class="bg-black/5 dark:bg-white/10 px-1 py-0.5 rounded text-xs">claude-opus-4-6</code> ，实际请求提供商用 <code class="bg-black/5 dark:bg-white/10 px-1 py-0.5 rounded text-xs">claude-sonnet-4-6</code>。</p>
      </div>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mt-6">
        <img
          loading="lazy"
          src="/guide/concepts-model-mapping-1.webp"
          alt="模型映射 1"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
        <img
          loading="lazy"
          src="/guide/concepts-model-mapping-2.webp"
          alt="模型映射 2"
          class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
        >
      </div>
    </section>

    <!-- 8. 反向代理 -->
    <section
      id="reverse-proxy"
      class="scroll-mt-24 lg:scroll-mt-20"
    >
      <h2>8. 反向代理</h2>
      <div class="space-y-6 mt-6">
        <div>
          <h4 class="font-medium text-[#262624] dark:text-[#f1ead8] mb-3">
            1. Codex
          </h4>
          <img
            loading="lazy"
            src="/guide/concepts-reverse-proxy-codex.webp"
            alt="Codex 反向代理"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full max-w-2xl"
          >
        </div>
        <div>
          <h4 class="font-medium text-[#262624] dark:text-[#f1ead8] mb-3">
            2. Krio
          </h4>
          <img
            loading="lazy"
            src="/guide/concepts-reverse-proxy-kiro.webp"
            alt="Krio 反向代理"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full max-w-2xl"
          >
        </div>
        <div>
          <h4 class="font-medium text-[#262624] dark:text-[#f1ead8] mb-3">
            3. Antigravity
          </h4>
          <img
            loading="lazy"
            src="/guide/concepts-reverse-proxy-antigravity.webp"
            alt="Antigravity 反向代理"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full max-w-2xl"
          >
        </div>
      </div>
    </section>

    <!-- 9. 优先级管理 -->
    <section
      id="priority-management"
      class="scroll-mt-24 lg:scroll-mt-20 pb-8"
    >
      <h2>9. 优先级管理</h2>
      <p class="text-sm text-[#666663] dark:text-[#a3a094] mb-4">
        允许拖动、或者直接点击数字输入调整优先级。
      </p>
      
      <div class="grid grid-cols-1 md:grid-cols-2 gap-6 mt-6">
        <div class="space-y-4">
          <div class="bg-white/50 dark:bg-white/5 p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] shadow-sm">
            <h4 class="font-bold text-[#262624] dark:text-[#f1ead8] flex items-center gap-2">
              1. 提供商优先
            </h4>
            <p class="text-sm text-[#666663] dark:text-[#a3a094] mt-1">
              按提供商顺序调度，同优先级负载均衡。
            </p>
          </div>
          <div class="bg-white/50 dark:bg-white/5 p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] shadow-sm">
            <h4 class="font-bold text-[#262624] dark:text-[#f1ead8] flex items-center gap-2">
              2. Key优先
            </h4>
            <p class="text-sm text-[#666663] dark:text-[#a3a094] mt-1">
              全局Key统一调度，同优先级负载均衡。
            </p>
          </div>
          <div class="bg-white/50 dark:bg-white/5 p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] shadow-sm">
            <h4 class="font-bold text-[#262624] dark:text-[#f1ead8] flex items-center gap-2">
              3. 缓存亲和模式
            </h4>
            <p class="text-sm text-[#666663] dark:text-[#a3a094] mt-1">
              在Key TTL时间的约束下，优先使用上一次请求使用的Key。
            </p>
          </div>
          <div class="bg-white/50 dark:bg-white/5 p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] shadow-sm">
            <h4 class="font-bold text-[#262624] dark:text-[#f1ead8] flex items-center gap-2">
              4. 负载均衡模式
            </h4>
            <ul class="list-disc pl-5 mt-1 text-sm text-[#666663] dark:text-[#a3a094] space-y-1">
              <li>取消全局提供商优先级，提供商内部Key依然保持优先级。</li>
              <li>取消全局Key优先级，乱序使用。</li>
            </ul>
          </div>
          <div class="bg-white/50 dark:bg-white/5 p-4 rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.06)] shadow-sm">
            <h4 class="font-bold text-[#262624] dark:text-[#f1ead8] flex items-center gap-2">
              5. 固定顺序模式
            </h4>
            <p class="text-sm text-[#666663] dark:text-[#a3a094] mt-1">
              取消缓存亲和，始终使用固定顺序Key请求。
            </p>
          </div>
        </div>
        
        <div class="space-y-4">
          <img
            loading="lazy"
            src="/guide/concepts-priority-1.webp"
            alt="优先级管理 1"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
          <img
            loading="lazy"
            src="/guide/concepts-priority-2.webp"
            alt="优先级管理 2"
            class="rounded-xl border border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)] shadow-sm w-full"
          >
        </div>
      </div>
    </section>
  </div>
</template>
