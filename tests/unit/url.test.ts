import { describe, it, expect } from 'vitest';

// URL 工具函数测试
describe('URL 处理', () => {
  describe('URL 规范化', () => {
    it('应添加 https:// 前缀到无协议域名', () => {
      const url = 'example.com';
      const normalized = url.match(/^https?:\/\//) ? url : 'https://' + url;
      expect(normalized).toBe('https://example.com');
    });

    it('应保留已有协议', () => {
      const url = 'http://example.com';
      const normalized = url.match(/^https?:\/\//) ? url : 'https://' + url;
      expect(normalized).toBe('http://example.com');
    });

    it('应处理 localhost', () => {
      const url = 'localhost:3000';
      const normalized = url.match(/^https?:\/\//) ? url : 'http://' + url;
      expect(normalized).toBe('http://localhost:3000');
    });
  });

  describe('URL 验证', () => {
    it('应识别有效URL', () => {
      const validUrls = ['https://example.com', 'http://localhost:3000'];
      validUrls.forEach(url => {
        expect(url.match(/^https?:\/\//)).toBeTruthy();
      });
    });

    it('应拒绝无效URL', () => {
      const invalid = 'ht!tp://invalid';
      expect(invalid.match(/^https?:\/\//)).toBeNull();
    });
  });
});
