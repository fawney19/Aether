export function installRequestTemplate(): HTMLTemplateElement {
  const existing = document.getElementById("requestTemplate");
  if (existing instanceof HTMLTemplateElement) return existing;

  const template = document.createElement("template");
  template.id = "requestTemplate";
  template.innerHTML = `
    <article class="request">
      <div class="request-title"><span class="request-icon" aria-hidden="true">!</span><strong class="request-method"></strong><span class="request-risk"></span><span class="request-id"></span></div>
      <p class="request-summary"></p>
      <pre class="request-command"></pre>
      <div class="request-questions"></div>
      <label class="request-scope-wrap" hidden>
        <span>授权范围</span>
        <select class="request-scope">
          <option value="turn">仅本次 turn</option>
          <option value="session">当前会话</option>
        </select>
      </label>
      <details class="request-details">
        <summary>查看请求数据</summary>
        <pre class="request-json"></pre>
      </details>
      <textarea class="request-response" rows="4" aria-label="JSON 响应"></textarea>
      <div class="button-row request-actions">
        <button class="primary request-allow">允许</button>
        <button class="secondary request-deny">拒绝</button>
        <button class="secondary request-send">发送 JSON</button>
      </div>
    </article>
  `;
  document.body.append(template);
  return template;
}
