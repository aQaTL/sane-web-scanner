<template>
	<div>
		<NuxtLink to="/fun">Fun page</NuxtLink>
		<NuxtLink to="/blog">Blog</NuxtLink>
		<Button v-on:click="ping">Ping</Button>
	</div>
</template>

<script lang="ts">
import Vue from 'vue'
import { Context } from "@nuxt/types";

export default Vue.extend({
		name: "index",

		head: function() {
			return {
				title: "sane-web-scanner"
			};
		},

		mounted: function() {
			this.ping();
		},

		methods: {
			ping: async function() {
				console.log(this.$nuxt.$options.context.isDev ? "debug mode" : "release mode");

				const api_endpoint = this.$nuxt.$options.context.isDev ? "http://localhost:8000/api/v1"
						: "/api/v1";


				try {
					const echo = await this.$nuxt.$options.context.app.$http.$get(
						`${api_endpoint}/ping`
					);
					console.log("echo resp:");
					console.log(echo);

				} catch (e) {
					// context.error(e);
					console.error(e);
				}
			}
		}
})

</script>
