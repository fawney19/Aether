-- Name: ldap_configs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ldap_configs ALTER COLUMN id SET DEFAULT nextval('public.ldap_configs_id_seq'::regclass);



--
-- Name: proxy_node_events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.proxy_node_events ALTER COLUMN id SET DEFAULT nextval('public.proxy_node_events_id_seq'::regclass);



--
